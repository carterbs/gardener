use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::task_identity::{compute_task_id, TaskIdentity};

use super::types::{NewTask, TaskStatus, WriteCmd};

pub(super) fn write_cmd_details(cmd: &WriteCmd) -> (&'static str, serde_json::Value) {
    match cmd {
        WriteCmd::Upsert { task, now, .. } => (
            "upsert",
            json!({
                "task_id": compute_task_id_from_new_task(task),
                "scope_key": task.scope_key,
                "priority": task.priority.as_str(),
                "source": task.source,
                "now": now,
            }),
        ),
        WriteCmd::InsertManualTask { task, now, .. } => (
            "insert_manual_task",
            json!({
                "task_id": task.task_id,
                "scope_key": task.scope_key,
                "priority": task.priority.as_str(),
                "status": task.status.as_str(),
                "source": task.source,
                "now": now,
            }),
        ),
        WriteCmd::ClaimNext {
            lease_owner,
            lease_expires_at,
            now,
            ..
        } => (
            "claim_next",
            json!({
                "lease_owner": lease_owner,
                "lease_expires_at": lease_expires_at,
                "now": now,
            }),
        ),
        WriteCmd::MarkInProgress {
            task_id,
            lease_owner,
            now,
            ..
        } => (
            "mark_in_progress",
            json!({
                "task_id": task_id,
                "lease_owner": lease_owner,
                "now": now,
            }),
        ),
        WriteCmd::MarkComplete {
            task_id,
            lease_owner,
            now,
            ..
        } => (
            "mark_complete",
            json!({
                "task_id": task_id,
                "lease_owner": lease_owner,
                "now": now,
            }),
        ),
        WriteCmd::RecoverStale { now, .. } => ("recover_stale", json!({ "now": now })),
        WriteCmd::ReleaseLease {
            task_id,
            lease_owner,
            now,
            ..
        } => (
            "release_lease",
            json!({
                "task_id": task_id,
                "lease_owner": lease_owner,
                "now": now,
            }),
        ),
        WriteCmd::MarkUnresolved {
            task_id,
            lease_owner,
            now,
            ..
        } => (
            "mark_unresolved",
            json!({
                "task_id": task_id,
                "lease_owner": lease_owner,
                "now": now,
            }),
        ),
        WriteCmd::SetUnresolvedReady { task_id, now, .. } => (
            "set_unresolved_ready",
            json!({
                "task_id": task_id,
                "now": now,
            }),
        ),
        WriteCmd::SetUnresolvedMergePending { task_id, now, .. } => (
            "set_unresolved_merge_pending",
            json!({
                "task_id": task_id,
                "now": now,
            }),
        ),
        WriteCmd::SetMergePendingReady { task_id, now, .. } => (
            "set_merge_pending_ready",
            json!({
                "task_id": task_id,
                "now": now,
            }),
        ),
        WriteCmd::ClearRelatedPr { task_id, .. } => {
            ("clear_related_pr", json!({ "task_id": task_id }))
        }
        WriteCmd::MarkMergePending {
            task_id,
            lease_owner,
            now,
            ..
        } => (
            "mark_merge_pending",
            json!({
                "task_id": task_id,
                "lease_owner": lease_owner,
                "now": now,
            }),
        ),
        WriteCmd::ClaimMergePending {
            merge_worker_id,
            now,
            ..
        } => (
            "claim_merge_pending",
            json!({
                "merge_worker_id": merge_worker_id,
                "now": now,
            }),
        ),
        WriteCmd::SetRelatedPr {
            task_id,
            pr_number,
            branch,
            now,
            ..
        } => (
            "set_related_pr",
            json!({
                "task_id": task_id,
                "pr_number": pr_number,
                "branch": branch,
                "now": now,
            }),
        ),
        WriteCmd::PromoteReadyWithPr { now, .. } => {
            ("promote_ready_with_pr", json!({ "now": now }))
        }
        WriteCmd::ReopenCompleteToMergePending { task_id, now, .. } => (
            "reopen_complete_to_merge_pending",
            json!({
                "task_id": task_id,
                "now": now,
            }),
        ),
        WriteCmd::UpdateTaskMetadata {
            task_id,
            patch,
            now,
            ..
        } => (
            "update_task_metadata",
            json!({
                "task_id": task_id,
                "status": patch.status.map(TaskStatus::as_str),
                "rationale": patch.rationale,
                "related_pr": patch.related_pr,
                "related_branch": patch.related_branch,
                "clear_lease": patch.clear_lease,
                "now": now,
            }),
        ),
        WriteCmd::InsertRejectedSeed {
            title, domain, now, ..
        } => (
            "insert_rejected_seed",
            json!({
                "title": title,
                "domain": domain,
                "now": now,
            }),
        ),
    }
}

pub(super) fn log_write_result(
    path: &Path,
    operation: &str,
    ok_payload: &Result<serde_json::Value, &GardenerError>,
    maybe_error: Option<&GardenerError>,
) {
    match maybe_error {
        Some(error) => {
            append_run_log(
                "error",
                "backlog_store.write_command.failed",
                json!({
                    "operation": operation,
                    "error": error.to_string(),
                    "path": path.display().to_string(),
                    "path_state": backlog_path_state(path),
                }),
            );
        }
        None => {
            append_run_log(
                "info",
                "backlog_store.write_command.applied",
                json!({
                    "operation": operation,
                    "result": ok_payload.as_ref().ok(),
                    "path": path.display().to_string(),
                    "path_state": backlog_path_state(path),
                }),
            );
        }
    }
}

pub(crate) fn backlog_path_state(path: &Path) -> serde_json::Value {
    append_run_log(
        "debug",
        "backlog_store.path_state.inspect",
        json!({
            "path": path.display().to_string(),
        }),
    );
    let file = |p: &Path| match std::fs::metadata(p) {
        Ok(meta) => {
            let modified_unix_ms = meta
                .modified()
                .ok()
                .and_then(|ts| ts.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64);
            json!({
                "exists": true,
                "size_bytes": meta.len(),
                "modified_unix_ms": modified_unix_ms,
            })
        }
        Err(_) => json!({
            "exists": false,
        }),
    };

    let wal_path = path.with_extension("sqlite-wal");
    let shm_path = path.with_extension("sqlite-shm");
    let bak_path = path.with_extension("sqlite.bak");
    json!({
        "main": {
            "path": path.display().to_string(),
            "meta": file(path),
        },
        "wal": {
            "path": wal_path.display().to_string(),
            "meta": file(&wal_path),
        },
        "shm": {
            "path": shm_path.display().to_string(),
            "meta": file(&shm_path),
        },
        "backup": {
            "path": bak_path.display().to_string(),
            "meta": file(&bak_path),
        },
    })
}

pub(super) fn compute_task_id_from_new_task(task: &NewTask) -> String {
    compute_task_id(TaskIdentity {
        kind: task.kind,
        title: task.title.clone(),
        scope_key: task.scope_key.clone(),
        related_pr: task.related_pr,
        related_branch: task.related_branch.clone(),
    })
}

pub(super) fn db_err(error: rusqlite::Error) -> GardenerError {
    GardenerError::Database(error.to_string())
}

pub fn system_time_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_else(|_| {
            append_run_log(
                "error",
                "backlog_store.system_time_zero",
                json!({ "reason": "SystemTime::duration_since(UNIX_EPOCH) failed; all timestamps will be 0" }),
            );
            0
        })
}
