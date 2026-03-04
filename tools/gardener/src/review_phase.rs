use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::fsm::{ReviewVerdict, ReviewingOutput};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::PromptRegistry;
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde::Serialize;
use serde_json::json;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ReviewContext<'a> {
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
    pub pr_number: u64,
    pub branch: &'a str,
    pub task_id: &'a str,
    #[allow(clippy::type_complexity)]
    pub on_step: Option<&'a dyn Fn(&str, &str)>,
    pub on_agent_event: Option<&'a dyn Fn(&crate::protocol::AgentEvent)>,
}

pub struct ReviewOutcome {
    pub verdict: ReviewVerdict,
    pub suggestions: Vec<String>,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}

pub fn run_review(ctx: &ReviewContext<'_>) -> Result<ReviewOutcome, GardenerError> {
    append_run_log(
        "info",
        "review_phase.started",
        json!({ "worker_id": ctx.identity.worker_id, "pr_number": ctx.pr_number }),
    );
    if let Some(on_step) = ctx.on_step {
        on_step(
            "REVIEW",
            &format!("starting review of PR #{}", ctx.pr_number),
        );
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
        state: WorkerState::Reviewing,
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
            .unwrap_or("review phase failed")
            .to_string();
        return Err(GardenerError::Process(format!(
            "review phase agent failure: {reason}"
        )));
    }

    let reviewing_output = parse_reviewing_output(&result.payload);
    persist_review_artifact(ctx, &reviewing_output);

    if let Some(on_step) = ctx.on_step {
        on_step(
            "REVIEW",
            &format!(
                "verdict={:?}, {} suggestions",
                reviewing_output.verdict,
                reviewing_output.suggestions.len()
            ),
        );
    }

    Ok(ReviewOutcome {
        verdict: reviewing_output.verdict,
        suggestions: reviewing_output.suggestions,
        prompt_version: result.prompt_version,
        context_manifest_hash: result.context_manifest_hash,
    })
}

pub(crate) fn parse_reviewing_output(payload: &serde_json::Value) -> ReviewingOutput {
    let verdict = payload
        .get("verdict")
        .and_then(serde_json::Value::as_str)
        .map(|v| match v.to_ascii_lowercase().as_str() {
            "approve" => ReviewVerdict::Approve,
            "needs_changes" => ReviewVerdict::NeedsChanges,
            _ => ReviewVerdict::NeedsChanges,
        })
        .unwrap_or(ReviewVerdict::NeedsChanges);
    let suggestions = payload
        .get("suggestions")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ReviewingOutput {
        verdict,
        suggestions,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ReviewArtifact {
    task_id: String,
    worker_id: String,
    verdict: String,
    suggestions: Vec<String>,
    recorded_at_unix_ms: i64,
}

fn persist_review_artifact(ctx: &ReviewContext<'_>, reviewing_output: &ReviewingOutput) {
    let artifact = ReviewArtifact {
        task_id: ctx.task_id.to_string(),
        worker_id: ctx.identity.worker_id.clone(),
        verdict: match reviewing_output.verdict {
            ReviewVerdict::Approve => "approve",
            ReviewVerdict::NeedsChanges => "needs_changes",
        }
        .to_string(),
        suggestions: reviewing_output.suggestions.clone(),
        recorded_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    };
    let artifact_path = ctx
        .scope
        .working_dir
        .join(".cache/gardener/reviews")
        .join(format!("{}.json", ctx.task_id));
    if let Some(parent) = artifact_path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if let Ok(payload) = serde_json::to_string_pretty(&artifact) {
        if std::fs::write(&artifact_path, payload).is_ok() {
            append_run_log(
                "info",
                "worker.review.persisted",
                json!({
                    "task_id": ctx.task_id, "worker_id": ctx.identity.worker_id,
                    "verdict": artifact.verdict, "suggestions_count": artifact.suggestions.len(),
                    "path": artifact_path.display().to_string(),
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_reviewing_output;

    #[test]
    fn parse_reviewing_output_defaults_to_needs_changes_without_verdict() {
        let output = parse_reviewing_output(&serde_json::json!({}));
        assert_eq!(output.verdict, crate::fsm::ReviewVerdict::NeedsChanges);
        assert!(output.suggestions.is_empty());
    }

    #[test]
    fn parse_reviewing_output_preserves_needs_changes_and_suggestions() {
        let output = parse_reviewing_output(&serde_json::json!({
            "verdict": "needs_changes",
            "suggestions": ["first", 2, "third"],
        }));
        assert_eq!(output.verdict, crate::fsm::ReviewVerdict::NeedsChanges);
        assert_eq!(output.suggestions, vec!["first", "third"]);
    }

    #[test]
    fn parse_reviewing_output_is_fail_closed_for_missing_or_unknown_verdict() {
        let missing = parse_reviewing_output(&serde_json::json!({}));
        assert_eq!(missing.verdict, crate::fsm::ReviewVerdict::NeedsChanges);

        let unknown = parse_reviewing_output(&serde_json::json!({
            "verdict": "ship_it",
            "suggestions": ["looks good"],
        }));
        assert_eq!(unknown.verdict, crate::fsm::ReviewVerdict::NeedsChanges);
    }
}
