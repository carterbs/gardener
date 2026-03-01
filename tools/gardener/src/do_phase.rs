use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::fsm::DoingOutput;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::PromptRegistry;
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;

pub struct DoContext<'a> {
    pub cfg: &'a AppConfig,
    pub process_runner: &'a dyn ProcessRunner,
    pub scope: &'a RuntimeScope,
    pub worktree_path: &'a Path,
    pub factory: &'a AdapterFactory,
    pub registry: &'a PromptRegistry,
    pub learning_loop: &'a LearningLoop,
    pub identity: &'a WorkerIdentity,
    pub task_summary: &'a str,
    pub attempt_count: i64,
    #[allow(clippy::type_complexity)]
    pub on_step: Option<&'a dyn Fn(&str, &str)>,
    pub on_agent_event: Option<&'a dyn Fn(&crate::protocol::AgentEvent)>,
}

pub struct DoOutcome {
    pub summary: String,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}

pub fn run_do(ctx: &DoContext<'_>) -> Result<DoOutcome, GardenerError> {
    append_run_log(
        "info",
        "do_phase.started",
        json!({ "worker_id": ctx.identity.worker_id, "task_summary": ctx.task_summary }),
    );
    if let Some(on_step) = ctx.on_step {
        on_step("DO", "starting do phase");
    }

    let result = run_agent_turn(AgentTurnInput {
        cfg: ctx.cfg,
        process_runner: ctx.process_runner,
        scope: ctx.scope,
        worktree_path: ctx.worktree_path,
        factory: ctx.factory,
        registry: ctx.registry,
        learning_loop: ctx.learning_loop,
        identity: ctx.identity,
        state: WorkerState::Doing,
        task_summary: ctx.task_summary,
        attempt_count: ctx.attempt_count,
        prompt_override: None,
        on_event: ctx.on_agent_event,
    })?;

    if result.terminal == AgentTerminal::Failure {
        let reason = result
            .payload
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("do phase failed")
            .to_string();
        return Err(GardenerError::Process(format!(
            "do phase agent failure: {reason}"
        )));
    }

    let doing_output =
        parse_doing_output(&result.payload, &ctx.identity.worker_id, ctx.task_summary)?;

    if let Some(on_step) = ctx.on_step {
        on_step("DO", "do phase complete");
    }

    Ok(DoOutcome {
        summary: doing_output.summary,
        prompt_version: result.prompt_version,
        context_manifest_hash: result.context_manifest_hash,
    })
}

pub(crate) fn parse_doing_output(
    payload: &serde_json::Value,
    worker_id: &str,
    task_summary: &str,
) -> Result<DoingOutput, GardenerError> {
    if let Ok(parsed) = serde_json::from_value::<DoingOutput>(payload.clone()) {
        return Ok(parsed);
    }
    append_run_log(
        "error",
        "worker.doing.payload_invalid",
        json!({
            "worker_id": worker_id, "task_summary": task_summary, "payload": payload,
        }),
    );
    Err(GardenerError::Process(format!(
        "doing phase produced invalid payload (task will be marked unresolved): {payload}"
    )))
}

pub(crate) fn fallback_commit_message(task_summary: &str) -> String {
    let first_line = task_summary.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return "feat: implement requested changes".to_string();
    }
    let normalized = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let max_desc_len = 72usize.saturating_sub("feat: ".len());
    let desc = normalized.chars().take(max_desc_len).collect::<String>();
    if desc.is_empty() {
        "feat: implement requested changes".to_string()
    } else {
        format!("feat: {desc}")
    }
}
