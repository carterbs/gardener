use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::fsm::{TaskCategory, UnderstandOutput};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::PromptRegistry;
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;

pub struct UnderstandContext<'a> {
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

pub struct UnderstandOutcome {
    pub category: TaskCategory,
    pub reasoning: String,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}

pub fn run_understand(ctx: &UnderstandContext<'_>) -> Result<UnderstandOutcome, GardenerError> {
    append_run_log(
        "info",
        "understand_phase.started",
        json!({ "worker_id": ctx.identity.worker_id, "task_summary": ctx.task_summary }),
    );
    if let Some(on_step) = ctx.on_step {
        on_step("UNDERSTAND", "starting understand phase");
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
        state: WorkerState::Understand,
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
            .unwrap_or("understand phase failed")
            .to_string();
        return Err(GardenerError::Process(format!(
            "understand phase agent failure: {reason}"
        )));
    }

    let understand =
        parse_understand_output(&result.payload, &ctx.identity.worker_id, ctx.task_summary);
    if let Some(on_step) = ctx.on_step {
        on_step(
            "UNDERSTAND",
            &format!(
                "classified as {:?}: {}",
                understand.task_type, understand.reasoning
            ),
        );
    }

    Ok(UnderstandOutcome {
        category: understand.task_type,
        reasoning: understand.reasoning,
        prompt_version: result.prompt_version,
        context_manifest_hash: result.context_manifest_hash,
    })
}

pub(crate) fn parse_understand_output(
    payload: &serde_json::Value,
    worker_id: &str,
    task_summary: &str,
) -> UnderstandOutput {
    if let Ok(parsed) = serde_json::from_value::<UnderstandOutput>(payload.clone()) {
        return parsed;
    }
    let fallback = classify_task(task_summary);
    append_run_log(
        "warn",
        "worker.understand.payload_invalid",
        json!({
            "worker_id": worker_id, "task_summary": task_summary,
            "fallback_task_type": format!("{fallback:?}"), "payload": payload,
        }),
    );
    UnderstandOutput {
        task_type: fallback,
        reasoning: "fallback deterministic keyword classifier (invalid understand payload)"
            .to_string(),
    }
}

pub fn classify_task(task_summary: &str) -> TaskCategory {
    let lower = task_summary.to_ascii_lowercase();
    if lower.contains("bug") || lower.contains("fix") {
        TaskCategory::Bugfix
    } else if lower.contains("refactor") {
        TaskCategory::Refactor
    } else if lower.contains("feature")
        || lower.contains("build")
        || lower.contains("implement")
        || lower.contains("replace")
    {
        TaskCategory::Feature
    } else if lower.contains("infra") {
        TaskCategory::Infra
    } else if lower.contains("chore") {
        TaskCategory::Chore
    } else {
        TaskCategory::Task
    }
}

#[cfg(test)]
mod tests {
    use crate::fsm::TaskCategory;
    use super::{classify_task, parse_understand_output};

    #[test]
    fn parse_understand_output_falls_back_to_classifier_when_payload_invalid() {
        let output = parse_understand_output(
            &serde_json::json!({"foo": "bar"}),
            "worker-1",
            "refactor: move prompt registry to module",
        );
        assert_eq!(output.task_type, TaskCategory::Refactor);
        assert_eq!(
            output.reasoning,
            "fallback deterministic keyword classifier (invalid understand payload)"
        );
    }

    #[test]
    fn classify_build_and_implement_as_feature_for_planning() {
        assert_eq!(
            classify_task(
                "GARD-04: Build Triage mode — Live activity and Triage artifacts cards"
            ),
            crate::fsm::TaskCategory::Feature
        );
        assert_eq!(
            classify_task(
                "GARD-02: Implement global frame — header, footer, and mode switching"
            ),
            crate::fsm::TaskCategory::Feature
        );
    }
}
