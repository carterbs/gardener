use crate::agent_turn::AgentTurnOutput;
use crate::fsm::ReviewingOutput;
use crate::logging::{
    append_run_log, current_run_id, current_run_log_path, recent_worker_log_lines,
};
use crate::types::{RuntimeScope, WorkerState};
use crate::worker::types::WorkerLogEvent;
use crate::worker::worktree_naming::worktree_slug_for_task;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HandoffRunEvidenceBundle {
    task_id: String,
    task_summary: String,
    attempt_count: i64,
    worker_id: String,
    session_id: String,
    branch: String,
    run_id: String,
    run_log_path: Option<String>,
    worker_log_events: Vec<WorkerLogEvent>,
    recent_worker_log_lines: Vec<String>,
    recorded_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReviewArtifact {
    task_id: String,
    worker_id: String,
    verdict: String,
    suggestions: Vec<String>,
    recorded_at_unix_ms: i64,
}

fn handoff_evidence_bundle_path(
    scope: &RuntimeScope,
    task_id: &str,
    run_id: &str,
) -> std::path::PathBuf {
    scope
        .working_dir
        .join(".cache/gardener/run-evidence-bundles")
        .join(format!(
            "{}-{}.json",
            worktree_slug_for_task(task_id),
            run_id
        ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_handoff_evidence_bundle(
    scope: &RuntimeScope,
    task_id: &str,
    task_summary: &str,
    attempt_count: i64,
    worker_id: &str,
    session_id: &str,
    branch: &str,
    logs: &[WorkerLogEvent],
) -> Option<std::path::PathBuf> {
    let run_id = current_run_id().unwrap_or_else(|| "no-run-id".to_string());
    let recent_lines = recent_worker_log_lines(worker_id, 250);
    let run_log_path = current_run_log_path();

    let evidence = HandoffRunEvidenceBundle {
        task_id: task_id.to_string(),
        task_summary: task_summary.to_string(),
        attempt_count,
        worker_id: worker_id.to_string(),
        session_id: session_id.to_string(),
        branch: branch.to_string(),
        run_id: run_id.clone(),
        run_log_path: run_log_path.as_ref().map(|path| path.display().to_string()),
        worker_log_events: logs.to_vec(),
        recent_worker_log_lines: recent_lines,
        recorded_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    };
    let artifact_path = handoff_evidence_bundle_path(scope, task_id, &run_id);
    if let Some(parent) = artifact_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            append_run_log(
                "warn",
                "worker.handoff_evidence.persist_failed",
                serde_json::json!({
                    "task_id": task_id,
                    "worker_id": worker_id,
                    "path": artifact_path.display().to_string(),
                    "error": err.to_string(),
                }),
            );
            return None;
        }
    }
    match serde_json::to_string_pretty(&evidence) {
        Ok(payload) => {
            if let Err(err) = std::fs::write(&artifact_path, payload) {
                append_run_log(
                    "warn",
                    "worker.handoff_evidence.persist_failed",
                    serde_json::json!({
                        "task_id": task_id,
                        "worker_id": worker_id,
                        "path": artifact_path.display().to_string(),
                        "error": err.to_string(),
                    }),
                );
                return None;
            }
            append_run_log(
                "info",
                "worker.handoff_evidence.persisted",
                serde_json::json!({
                    "task_id": task_id,
                    "worker_id": worker_id,
                    "path": artifact_path.display().to_string(),
                }),
            );
            Some(artifact_path)
        }
        Err(err) => {
            append_run_log(
                "warn",
                "worker.handoff_evidence.persist_failed",
                serde_json::json!({
                    "task_id": task_id,
                    "worker_id": worker_id,
                    "error": err.to_string(),
                }),
            );
            None
        }
    }
}

pub(crate) fn log_and_persist_review_output(
    scope: &RuntimeScope,
    task_id: &str,
    worker_id: &str,
    reviewing_output: &ReviewingOutput,
) {
    let artifact = ReviewArtifact {
        task_id: task_id.to_string(),
        worker_id: worker_id.to_string(),
        verdict: match reviewing_output.verdict {
            crate::fsm::ReviewVerdict::Approve => "approve".to_string(),
            crate::fsm::ReviewVerdict::NeedsChanges => "needs_changes".to_string(),
        },
        suggestions: reviewing_output.suggestions.clone(),
        recorded_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    };
    let artifact_path = review_artifact_path(scope, task_id);
    if let Some(parent) = artifact_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            append_run_log(
                "warn",
                "worker.review.persist_failed",
                serde_json::json!({
                    "task_id": task_id,
                    "worker_id": worker_id,
                    "path": artifact_path.display().to_string(),
                    "error": err.to_string(),
                }),
            );
            return;
        }
    }
    match serde_json::to_string_pretty(&artifact) {
        Ok(payload) => {
            if let Err(err) = std::fs::write(&artifact_path, payload) {
                append_run_log(
                    "warn",
                    "worker.review.persist_failed",
                    serde_json::json!({
                        "task_id": task_id,
                        "worker_id": worker_id,
                        "path": artifact_path.display().to_string(),
                        "error": err.to_string(),
                    }),
                );
                return;
            }
            append_run_log(
                "info",
                "worker.review.persisted",
                serde_json::json!({
                    "task_id": task_id,
                    "worker_id": worker_id,
                    "verdict": artifact.verdict,
                    "suggestions_count": artifact.suggestions.len(),
                    "path": artifact_path.display().to_string(),
                }),
            );
        }
        Err(err) => {
            append_run_log(
                "warn",
                "worker.review.persist_failed",
                serde_json::json!({
                    "task_id": task_id,
                    "worker_id": worker_id,
                    "path": artifact_path.display().to_string(),
                    "error": err.to_string(),
                }),
            );
        }
    }
}

pub(crate) fn review_artifact_path(scope: &RuntimeScope, task_id: &str) -> std::path::PathBuf {
    scope
        .working_dir
        .join(".cache/gardener/reviews")
        .join(format!("{}.json", worktree_slug_for_task(task_id)))
}

pub(crate) fn log_event_from(output: &AgentTurnOutput, state: WorkerState) -> WorkerLogEvent {
    WorkerLogEvent {
        state,
        prompt_version: output.prompt_version.clone(),
        context_manifest_hash: output.context_manifest_hash.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{collect_handoff_evidence_bundle, review_artifact_path};
    use crate::logging;
    use crate::types::{RuntimeScope, WorkerState};
    use crate::worker::types::WorkerLogEvent;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn review_artifact_path_is_task_scoped_and_git_safe() {
        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/repo"),
            repo_root: Some(PathBuf::from("/repo")),
            working_dir: PathBuf::from("/repo"),
        };
        let path = review_artifact_path(&scope, "manual:tui:GARD-01");
        assert_eq!(
            path.display().to_string(),
            format!(
                "/repo/.cache/gardener/reviews/{}.json",
                super::worktree_slug_for_task("manual:tui:GARD-01")
            )
        );
    }

    #[test]
    fn collect_handoff_evidence_bundle_persists_runnable_artifact() {
        logging::clear_run_logger();
        let task_id = "manual:tui:GARD-01";
        let dir = tempfile::tempdir().expect("tempdir");
        let scope = RuntimeScope {
            process_cwd: dir.path().to_path_buf(),
            repo_root: Some(dir.path().to_path_buf()),
            working_dir: dir.path().to_path_buf(),
        };
        let log_path = scope.working_dir.join(".gardener/otel-logs.jsonl");
        let run_id = logging::init_run_logger(&log_path, &scope.working_dir);
        logging::append_run_log(
            "info",
            "worker.task.started",
            json!({"worker_id":"worker-1","task_id": task_id}),
        );
        logging::append_run_log(
            "info",
            "worker.review.approved",
            json!({"worker_id":"worker-1","task_id": task_id}),
        );

        let bundle_path = collect_handoff_evidence_bundle(
            &scope,
            task_id,
            "Add test evidence bundle",
            2,
            "worker-1",
            "session-1",
            &super::worktree_slug_for_task(task_id),
            &[WorkerLogEvent {
                state: WorkerState::Reviewing,
                prompt_version: "prompt-v1".to_string(),
                context_manifest_hash: "a".repeat(64),
            }],
        )
        .expect("bundle persisted");
        assert_eq!(
            bundle_path,
            super::handoff_evidence_bundle_path(&scope, task_id, &run_id)
        );
        let payload = std::fs::read_to_string(&bundle_path).expect("artifact read");
        let parsed = serde_json::from_str::<serde_json::Value>(&payload).expect("bundle json");
        assert_eq!(parsed["task_id"], task_id);
        assert_eq!(parsed["run_id"], run_id);
        assert_eq!(parsed["worker_id"], "worker-1");
        assert!(parsed["recent_worker_log_lines"].as_array().is_some());
        logging::clear_run_logger();
    }
}
