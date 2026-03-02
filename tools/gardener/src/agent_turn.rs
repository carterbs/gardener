use crate::agent::factory::AdapterFactory;
use crate::agent::AdapterContext;
use crate::config::{effective_agent_for_state, effective_model_for_state, AppConfig};
use crate::errors::GardenerError;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::output_envelope::{parse_typed_payload, END_MARKER, START_MARKER};
use crate::prompt_context::PromptContextItem;
use crate::prompt_knowledge::to_prompt_lines;
use crate::prompt_registry::{PromptRegistry, PromptTemplate};
use crate::prompts::{render_prompt_with_body, render_state_prompt};
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;

pub struct AgentTurnInput<'a> {
    pub cfg: &'a AppConfig,
    pub process_runner: &'a dyn ProcessRunner,
    pub scope: &'a RuntimeScope,
    pub worktree_path: &'a Path,
    pub factory: &'a AdapterFactory,
    pub registry: &'a PromptRegistry,
    pub learning_loop: &'a LearningLoop,
    pub identity: &'a WorkerIdentity,
    pub state: WorkerState,
    pub task_summary: &'a str,
    pub attempt_count: i64,
    pub prompt_override: Option<&'a PromptTemplate>,
    #[allow(clippy::type_complexity)]
    pub on_event: Option<&'a dyn Fn(&crate::protocol::AgentEvent)>,
}

pub struct AgentTurnOutput {
    pub terminal: AgentTerminal,
    pub payload: serde_json::Value,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}

pub fn run_agent_turn(input: AgentTurnInput<'_>) -> Result<AgentTurnOutput, GardenerError> {
    let AgentTurnInput {
        cfg,
        process_runner,
        scope,
        worktree_path,
        factory,
        registry,
        learning_loop,
        identity,
        state,
        task_summary,
        attempt_count,
        prompt_override,
        on_event,
    } = input;

    let prepared = prepare_prompt(
        cfg,
        registry,
        learning_loop,
        state,
        &identity.worker_id,
        task_summary,
        attempt_count,
        prompt_override,
    )?;

    let backend = effective_agent_for_state(cfg, state).ok_or_else(|| {
        GardenerError::InvalidConfig(format!("no backend configured for {state:?}"))
    })?;
    let model = effective_model_for_state(cfg, state);
    let adapter = factory.get(backend).ok_or_else(|| {
        GardenerError::InvalidConfig(format!("adapter not registered for {:?}", backend))
    })?;

    let output_file = scope.working_dir.join(format!(
        ".cache/gardener/worker-output-{}-{}.json",
        identity.worker_id,
        state.as_str()
    ));
    if let Some(parent) = output_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GardenerError::Io(e.to_string()))?;
    }

    let estimated_prompt_tokens = prepared.rendered.split_whitespace().count();
    append_run_log(
        "info",
        "agent.turn.started",
        json!({
            "worker_id": identity.worker_id,
            "session_id": identity.session.session_id,
            "state": state.as_str(),
            "backend": backend.as_str(),
            "model": model,
            "worktree": worktree_path.display().to_string(),
            "output_file": output_file.display().to_string(),
            "initial_prompt_est_tokens": estimated_prompt_tokens
        }),
    );
    crate::logging::append_run_log_untruncated(
        "info",
        "agent.turn.prompt",
        json!({
            "worker_id": identity.worker_id,
            "session_id": identity.session.session_id,
            "state": state.as_str(),
            "prompt": prepared.rendered
        }),
    );

    let output_schema = if state == WorkerState::Doing {
        let caps = adapter
            .probe_capabilities(process_runner)
            .unwrap_or_default();
        if caps.supports_output_schema {
            let schema_path = scope
                .working_dir
                .join(".cache/gardener/doing-output-schema.json");
            let schema = r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"summary":{"type":"string"}},"required":["summary"],"additionalProperties":false}"#;
            std::fs::write(&schema_path, schema)
                .ok()
                .map(|_| schema_path)
        } else {
            None
        }
    } else {
        None
    };

    let max_turns = Some(max_turns_for_state(cfg, state));
    let ctx = AdapterContext {
        worker_id: identity.worker_id.clone(),
        session_id: identity.session.session_id.clone(),
        sandbox_id: identity.session.sandbox_id.clone(),
        model,
        cwd: worktree_path.to_path_buf(),
        prompt_version: prepared.prompt_version.clone(),
        context_manifest_hash: prepared.context_manifest_hash.clone(),
        output_schema,
        output_file: Some(output_file),
        permissive_mode: cfg.execution.permissions_mode == "permissive_v1",
        max_turns,
    };
    let step = if let Some(cb) = on_event {
        let mut wrapper = |e: &crate::protocol::AgentEvent| cb(e);
        adapter.execute(process_runner, &ctx, &prepared.rendered, Some(&mut wrapper))?
    } else {
        adapter.execute(process_runner, &ctx, &prepared.rendered, None)?
    };

    append_run_log(
        if step.terminal == AgentTerminal::Success {
            "info"
        } else {
            "error"
        },
        "agent.turn.finished",
        json!({
            "worker_id": identity.worker_id,
            "session_id": identity.session.session_id,
            "state": state.as_str(),
            "terminal": match step.terminal {
                AgentTerminal::Success => "success",
                AgentTerminal::Failure => "failure"
            },
            "diagnostic_count": step.diagnostics.len()
        }),
    );

    Ok(AgentTurnOutput {
        terminal: step.terminal,
        payload: step.payload,
        prompt_version: prepared.prompt_version,
        context_manifest_hash: prepared.context_manifest_hash,
    })
}

// --- Prompt preparation ---

pub struct PreparedPrompt {
    pub prompt_version: String,
    pub context_manifest_hash: String,
    pub rendered: String,
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_prompt(
    cfg: &AppConfig,
    registry: &PromptRegistry,
    learning_loop: &LearningLoop,
    state: WorkerState,
    worker_id: &str,
    task_summary: &str,
    attempt_count: i64,
    prompt_override: Option<&PromptTemplate>,
) -> Result<PreparedPrompt, GardenerError> {
    append_run_log(
        "debug",
        "agent_turn.prompt.prepare",
        json!({
            "worker_id": worker_id,
            "state": state.as_str(),
            "knowledge_entries": learning_loop.entries().len(),
            "prompt_override": prompt_override.is_some()
        }),
    );
    let knowledge = to_prompt_lines(
        learning_loop.entries(),
        cfg.learning.deactivate_below_confidence,
    )
    .join("\n");

    let items = vec![
        ctx_item(
            "task_packet",
            "task",
            "task-hash",
            "task input",
            100,
            task_summary,
        ),
        ctx_item(
            "repo_context",
            "repo",
            "repo-hash",
            "repo snapshot",
            90,
            "repo context",
        ),
        ctx_item(
            "evidence_context",
            "evidence",
            "ev-hash",
            "evidence-ranked",
            80,
            "evidence context",
        ),
        ctx_item(
            "execution_context",
            "execution",
            "exec-hash",
            "state+identity",
            70,
            &format!(
                "state={state:?};backend={:?};attempt_count={attempt_count}",
                effective_agent_for_state(cfg, state)
            ),
        ),
        ctx_item(
            "knowledge_context",
            "knowledge",
            "know-hash",
            "learning loop",
            60,
            if knowledge.trim().is_empty() {
                "no prior knowledge"
            } else {
                &knowledge
            },
        ),
    ];

    let rendered = if let Some(tpl) = prompt_override {
        render_prompt_with_body(tpl.body, tpl.version, state, items)?
    } else {
        render_state_prompt(registry, state, items)?
    };

    let _parsed = parse_typed_payload::<serde_json::Value>(
        &format!(
            "{}{{\"schema_version\":1,\"state\":\"{}\",\"payload\":{{\"ok\":true}}}}{}",
            START_MARKER,
            state.as_str(),
            END_MARKER
        ),
        state,
    )?;

    let prompt_version = rendered.prompt_version;
    let context_manifest_hash = rendered.packet.context_manifest.manifest_hash;
    append_run_log(
        "debug",
        "agent_turn.prompt.ready",
        json!({
            "worker_id": worker_id,
            "state": state.as_str(),
            "prompt_version": prompt_version,
            "context_manifest_hash": context_manifest_hash
        }),
    );
    Ok(PreparedPrompt {
        prompt_version,
        context_manifest_hash,
        rendered: rendered.rendered,
    })
}

fn ctx_item(
    section: &str,
    source_id: &str,
    source_hash: &str,
    rationale: &str,
    rank: u32,
    content: &str,
) -> PromptContextItem {
    PromptContextItem {
        section: section.to_string(),
        source_id: source_id.to_string(),
        source_hash: source_hash.to_string(),
        rationale: rationale.to_string(),
        rank,
        content: content.to_string(),
    }
}

pub fn max_turns_for_state(cfg: &AppConfig, state: WorkerState) -> u32 {
    match state {
        WorkerState::Understand => cfg.prompts.turn_budget.understand,
        WorkerState::Planning => cfg.prompts.turn_budget.planning,
        WorkerState::Doing => cfg.prompts.turn_budget.doing,
        WorkerState::Gitting => cfg.prompts.turn_budget.gitting,
        WorkerState::Reviewing => cfg.prompts.turn_budget.reviewing,
        WorkerState::Merging => cfg.prompts.turn_budget.merging,
        WorkerState::Seeding
        | WorkerState::Complete
        | WorkerState::Failed
        | WorkerState::Parked => cfg.prompts.turn_budget.doing,
    }
}
