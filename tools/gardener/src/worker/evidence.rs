use crate::logging::{
    append_run_log, current_run_id, current_run_log_path, recent_worker_log_lines,
};
use crate::types::RuntimeScope;
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

#[cfg(test)]
mod tests {
    use super::collect_handoff_evidence_bundle;
    use crate::logging;
    use crate::types::{RuntimeScope, WorkerState};
    use crate::worker::types::WorkerLogEvent;
    use serde_json::json;

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
