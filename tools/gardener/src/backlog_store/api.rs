use serde_json::json;
use tokio::sync::oneshot;

use crate::errors::GardenerError;
use crate::logging::append_run_log;

use super::logging::{db_err, system_time_unix};
use super::queries::{fetch_task, row_to_task};
use super::types::{
    BacklogTask, ManualTaskInput, NewTask, RejectedSeed, StoreResult, TaskMutation, TaskStatus,
    TaskUpdatePatch, WriteCmd,
};
use super::BacklogStore;

impl BacklogStore {
    pub fn upsert_task(&self, task: NewTask) -> StoreResult<BacklogTask> {
        append_run_log(
            "debug",
            "backlog.task.upsert_client_started",
            json!({
                "scope_key": task.scope_key,
                "priority": task.priority.as_str(),
                "source": task.source,
            }),
        );
        let now = system_time_unix();
        append_run_log(
            "debug",
            "backlog.task.upsert",
            json!({
                "kind": task.kind.as_str(),
                "title": task.title,
                "scope_key": task.scope_key,
                "priority": task.priority.as_str(),
                "source": task.source,
            }),
        );
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::Upsert {
                task,
                now,
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        if let Ok(ref row) = result {
            append_run_log(
                "info",
                "backlog.task.upserted",
                json!({
                    "task_id": row.task_id,
                    "kind": row.kind.as_str(),
                    "title": row.title,
                    "scope_key": row.scope_key,
                    "priority": row.priority.as_str(),
                    "status": row.status.as_str(),
                    "source": row.source,
                }),
            );
        }
        result
    }

    pub fn insert_manual_task(&self, task: ManualTaskInput) -> StoreResult<BacklogTask> {
        let now = system_time_unix();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::InsertManualTask {
                task,
                now,
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?
    }

    pub fn claim_next(
        &self,
        lease_owner: &str,
        lease_duration_secs: i64,
    ) -> StoreResult<Option<BacklogTask>> {
        let now = system_time_unix();
        let lease_expires_at = now.saturating_add(lease_duration_secs.saturating_mul(1000));
        append_run_log(
            "debug",
            "backlog.task.claim_next",
            json!({
                "lease_owner": lease_owner,
                "lease_duration_secs": lease_duration_secs,
            }),
        );
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::ClaimNext {
                lease_owner: lease_owner.to_string(),
                lease_expires_at,
                now,
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(Some(task)) => {
                append_run_log(
                    "info",
                    "backlog.task.claimed",
                    json!({
                        "task_id": task.task_id,
                        "kind": task.kind.as_str(),
                        "title": task.title,
                        "priority": task.priority.as_str(),
                        "lease_owner": lease_owner,
                        "attempt_count": task.attempt_count,
                    }),
                );
            }
            Ok(None) => {
                append_run_log(
                    "debug",
                    "backlog.task.claim_next.empty",
                    json!({
                        "lease_owner": lease_owner,
                    }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.claim_next.failed",
                    json!({
                        "lease_owner": lease_owner,
                        "error": e.to_string(),
                    }),
                );
            }
        }
        result
    }

    pub fn mark_in_progress(&self, task_id: &str, lease_owner: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::MarkInProgress {
                task_id: task_id.to_string(),
                lease_owner: lease_owner.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.in_progress",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Ok(false) => {
                append_run_log(
                    "warn",
                    "backlog.task.in_progress.rejected",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.in_progress.failed",
                    json!({ "task_id": task_id, "lease_owner": lease_owner, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn mark_complete(&self, task_id: &str, lease_owner: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::MarkComplete {
                task_id: task_id.to_string(),
                lease_owner: lease_owner.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.completed",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Ok(false) => {
                append_run_log(
                    "warn",
                    "backlog.task.completed.rejected",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.completed.failed",
                    json!({ "task_id": task_id, "lease_owner": lease_owner, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn release_lease(&self, task_id: &str, lease_owner: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::ReleaseLease {
                task_id: task_id.to_string(),
                lease_owner: lease_owner.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.lease_released",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Ok(false) => {
                append_run_log(
                    "warn",
                    "backlog.task.lease_released.rejected",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.lease_released.failed",
                    json!({ "task_id": task_id, "lease_owner": lease_owner, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn mark_unresolved(&self, task_id: &str, lease_owner: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::MarkUnresolved {
                task_id: task_id.to_string(),
                lease_owner: lease_owner.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.unresolved",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Ok(false) => {
                append_run_log(
                    "warn",
                    "backlog.task.unresolved.rejected",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.unresolved.failed",
                    json!({ "task_id": task_id, "lease_owner": lease_owner, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn set_unresolved_to_ready(&self, task_id: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::SetUnresolvedReady {
                task_id: task_id.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.unresolved_to_ready",
                    json!({ "task_id": task_id }),
                );
            }
            Ok(false) => {
                append_run_log(
                    "warn",
                    "backlog.task.unresolved_to_ready.rejected",
                    json!({ "task_id": task_id }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.unresolved_to_ready.failed",
                    json!({ "task_id": task_id, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn set_unresolved_to_merge_pending(&self, task_id: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::SetUnresolvedMergePending {
                task_id: task_id.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.unresolved_to_merge_pending",
                    json!({ "task_id": task_id }),
                );
            }
            Ok(false) => {
                append_run_log(
                    "warn",
                    "backlog.task.unresolved_to_merge_pending.rejected",
                    json!({ "task_id": task_id }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.unresolved_to_merge_pending.failed",
                    json!({ "task_id": task_id, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn set_merge_pending_to_ready(&self, task_id: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::SetMergePendingReady {
                task_id: task_id.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.merge_pending_to_ready",
                    json!({ "task_id": task_id }),
                );
            }
            Ok(false) => {
                append_run_log(
                    "warn",
                    "backlog.task.merge_pending_to_ready.rejected",
                    json!({ "task_id": task_id }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.merge_pending_to_ready.failed",
                    json!({ "task_id": task_id, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn clear_related_pr(&self, task_id: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::ClearRelatedPr {
                task_id: task_id.to_string(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.related_pr_cleared",
                    json!({ "task_id": task_id }),
                );
            }
            Ok(false) => {
                append_run_log(
                    "warn",
                    "backlog.task.related_pr_cleared.none",
                    json!({ "task_id": task_id }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.related_pr_cleared.failed",
                    json!({ "task_id": task_id, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn mark_merge_pending(&self, task_id: &str, lease_owner: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::MarkMergePending {
                task_id: task_id.to_string(),
                lease_owner: lease_owner.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.merge_pending",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Ok(false) => {
                append_run_log(
                    "warn",
                    "backlog.task.merge_pending.rejected",
                    json!({ "task_id": task_id, "lease_owner": lease_owner }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.merge_pending.failed",
                    json!({ "task_id": task_id, "lease_owner": lease_owner, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn claim_merge_pending(&self, merge_worker_id: &str) -> StoreResult<Option<BacklogTask>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::ClaimMergePending {
                merge_worker_id: merge_worker_id.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(Some(task)) => {
                append_run_log(
                    "info",
                    "backlog.task.merge_claimed",
                    json!({ "task_id": task.task_id, "merge_worker_id": merge_worker_id }),
                );
            }
            Ok(None) => {}
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.merge_claimed.failed",
                    json!({ "merge_worker_id": merge_worker_id, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn set_related_pr(&self, task_id: &str, pr_number: i64, branch: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::SetRelatedPr {
                task_id: task_id.to_string(),
                pr_number,
                branch: branch.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.related_pr_set",
                    json!({
                        "task_id": task_id,
                        "pr_number": pr_number,
                        "branch": branch,
                    }),
                );
            }
            Ok(false) => {}
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.related_pr_set.failed",
                    json!({
                        "task_id": task_id,
                        "pr_number": pr_number,
                        "branch": branch,
                        "error": e.to_string(),
                    }),
                );
            }
        }
        result
    }

    pub fn promote_ready_with_pr(&self) -> StoreResult<usize> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::PromoteReadyWithPr {
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(count) if *count > 0 => {
                append_run_log(
                    "info",
                    "backlog.tasks.promoted_to_merge_pending",
                    json!({ "count": count }),
                );
            }
            Ok(_) => {}
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.tasks.promoted_to_merge_pending.failed",
                    json!({ "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn reopen_complete_to_merge_pending(&self, task_id: &str) -> StoreResult<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::ReopenCompleteToMergePending {
                task_id: task_id.to_string(),
                now: system_time_unix(),
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(true) => {
                append_run_log(
                    "info",
                    "backlog.task.reopened_to_merge_pending",
                    json!({ "task_id": task_id }),
                );
            }
            Ok(false) => {}
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.task.reopened_to_merge_pending.failed",
                    json!({ "task_id": task_id, "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn recover_stale_leases(&self, now: i64) -> StoreResult<usize> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::RecoverStale {
                now,
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        let result = reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        match &result {
            Ok(count) if *count > 0 => {
                append_run_log(
                    "warn",
                    "backlog.stale_leases.recovered",
                    json!({ "count": count }),
                );
            }
            Ok(count) => {
                append_run_log(
                    "debug",
                    "backlog.stale_leases.checked",
                    json!({ "count": count }),
                );
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "backlog.stale_leases.recovery_failed",
                    json!({ "error": e.to_string() }),
                );
            }
        }
        result
    }

    pub fn list_recent_tasks(&self, limit: usize) -> StoreResult<Vec<BacklogTask>> {
        self.read_pool.with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT task_id, kind, title, details, scope_key, priority, status, last_updated, \
                            lease_owner, lease_expires_at, source, related_pr, related_branch, rationale, attempt_count, created_at \
                     FROM backlog_tasks \
                     ORDER BY created_at DESC \
                     LIMIT ?1",
                )
                .map_err(db_err)?;
            let rows = statement
                .query_map([limit as i64], row_to_task)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            Ok(rows)
        })
    }

    pub fn list_tasks(&self) -> StoreResult<Vec<BacklogTask>> {
        self.read_pool.with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT task_id, kind, title, details, scope_key, priority, status, last_updated, \
                            lease_owner, lease_expires_at, source, related_pr, related_branch, rationale, attempt_count, created_at \
                     FROM backlog_tasks \
                     ORDER BY
                        CASE priority WHEN 'P0' THEN 0 WHEN 'P1' THEN 1 ELSE 2 END,
                        CASE WHEN attempt_count > 0 THEN 0 ELSE 1 END,
                        attempt_count DESC,
                        last_updated ASC,
                        created_at ASC",
                )
                .map_err(db_err)?;
            let rows = statement
                .query_map([], row_to_task)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            Ok(rows)
        })
    }

    pub fn list_backlog_tasks(&self) -> StoreResult<Vec<BacklogTask>> {
        self.read_pool.with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT task_id, kind, title, details, scope_key, priority, status, last_updated, \
                            lease_owner, lease_expires_at, source, related_pr, related_branch, rationale, attempt_count, created_at \
                     FROM backlog_tasks \
                     WHERE status != 'merge_pending' \
                     ORDER BY
                        CASE priority WHEN 'P0' THEN 0 WHEN 'P1' THEN 1 ELSE 2 END,
                        CASE WHEN attempt_count > 0 THEN 0 ELSE 1 END,
                        attempt_count DESC,
                        last_updated ASC,
                        created_at ASC",
                )
                .map_err(db_err)?;
            let rows = statement
                .query_map([], row_to_task)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            Ok(rows)
        })
    }

    pub fn count_tasks_by_priority(&self) -> StoreResult<(usize, usize, usize)> {
        append_run_log(
            "debug",
            "backlog.tasks.count_by_priority.started",
            json!({}),
        );
        self.read_pool.with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT
                        COALESCE(SUM(CASE WHEN priority = 'P0' THEN 1 ELSE 0 END), 0) AS p0,
                        COALESCE(SUM(CASE WHEN priority = 'P1' THEN 1 ELSE 0 END), 0) AS p1,
                        COALESCE(SUM(CASE WHEN priority = 'P2' THEN 1 ELSE 0 END), 0) AS p2
                     FROM backlog_tasks
                    WHERE status <> 'complete'",
                )
                .map_err(db_err)?;
            statement
                .query_row([], |row| {
                    let p0: i64 = row.get(0)?;
                    let p1: i64 = row.get(1)?;
                    let p2: i64 = row.get(2)?;
                    Ok((p0 as usize, p1 as usize, p2 as usize))
                })
                .map_err(db_err)
        })
    }

    pub fn count_active_tasks(&self) -> StoreResult<usize> {
        append_run_log("debug", "backlog.tasks.count_active.started", json!({}));
        self.read_pool.with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT COUNT(*) FROM backlog_tasks WHERE status NOT IN ('complete', 'failed')",
                )
                .map_err(db_err)?;
            statement
                .query_row([], |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count as usize)
                })
                .map_err(db_err)
        })
    }

    pub fn get_task(&self, task_id: &str) -> StoreResult<Option<BacklogTask>> {
        self.read_pool.with_conn(|conn| fetch_task(conn, task_id))
    }

    pub fn update_task_metadata(
        &self,
        task_id: &str,
        patch: TaskUpdatePatch,
    ) -> StoreResult<TaskMutation> {
        let now = system_time_unix();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::UpdateTaskMetadata {
                task_id: task_id.to_string(),
                patch,
                now,
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?
    }

    pub fn retire_task(
        &self,
        task_id: &str,
        status: TaskStatus,
        rationale: String,
        related_pr: Option<i64>,
        related_branch: Option<String>,
        clear_lease: bool,
    ) -> StoreResult<TaskMutation> {
        self.update_task_metadata(
            task_id,
            TaskUpdatePatch {
                status: Some(status),
                rationale: Some(rationale),
                related_pr,
                related_branch,
                clear_lease,
            },
        )
    }

    pub fn insert_rejected_seed(
        &self,
        task: &crate::seed_runner::SeedTask,
        reason: Option<&str>,
    ) -> StoreResult<()> {
        let now = system_time_unix();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender()?
            .blocking_send(WriteCmd::InsertRejectedSeed {
                title: task.title.clone(),
                details: task.details.clone(),
                rationale: task.rationale.clone(),
                domain: task.domain.clone(),
                priority: task.priority.clone(),
                rejection_reason: reason.unwrap_or("").to_string(),
                now,
                reply: reply_tx,
            })
            .map_err(|e| GardenerError::Database(e.to_string()))?;
        reply_rx
            .blocking_recv()
            .map_err(|e| GardenerError::Database(e.to_string()))?
    }

    pub fn list_rejected_seeds(&self) -> StoreResult<Vec<RejectedSeed>> {
        self.read_pool.with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT title, details, rejection_reason, domain \
                     FROM rejected_seed_tasks \
                     ORDER BY rejected_at DESC \
                     LIMIT 20",
                )
                .map_err(db_err)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(RejectedSeed {
                        title: row.get(0)?,
                        details: row.get(1)?,
                        rejection_reason: row.get(2)?,
                        domain: row.get(3)?,
                    })
                })
                .map_err(db_err)?;
            let mut seeds = Vec::new();
            for row in rows {
                seeds.push(row.map_err(db_err)?);
            }
            Ok(seeds)
        })
    }
}
