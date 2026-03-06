mod api;
mod logging;
mod migrations;
mod mutations;
mod queries;
#[cfg(test)]
mod tests;
mod types;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use rusqlite::{params, Connection, OpenFlags};
use serde_json::json;
use tokio::sync::mpsc;

use crate::errors::GardenerError;
use crate::logging::append_run_log;

use self::logging::{
    compute_task_id_from_new_task, db_err, log_write_result, write_cmd_details,
};
use self::migrations::run_migrations;
use self::mutations::{
    claim_merge_pending, claim_next, clear_related_pr, insert_manual_task, mark_complete,
    mark_in_progress, mark_merge_pending, mark_unresolved, promote_ready_with_pr, recover_stale,
    release_lease, reopen_complete_to_merge_pending, set_merge_pending_to_ready, set_related_pr,
    set_unresolved_status, update_task_metadata, upsert_task,
};
use self::queries::fetch_task;
use self::types::{StoreResult, WriteCmd};

pub use self::logging::system_time_unix;
pub use self::types::{
    BacklogTask, ManualTaskInput, NewTask, RejectedSeed, TaskMutation, TaskStatus,
    TaskUpdatePatch,
};
pub(crate) use self::logging::backlog_path_state;

const READ_POOL_SIZE: usize = 4;

pub struct BacklogStore {
    write_tx: Option<mpsc::Sender<WriteCmd>>,
    pub(super) read_pool: ReadPool,
    writer_join: Option<thread::JoinHandle<()>>,
    db_path: PathBuf,
}

impl Drop for BacklogStore {
    fn drop(&mut self) {
        drop(self.write_tx.take());
        if let Some(handle) = self.worker_join_handle() {
            let _ = handle.join();
        }
    }
}

impl BacklogStore {
    fn worker_join_handle(&mut self) -> Option<thread::JoinHandle<()>> {
        self.writer_join.take()
    }

    fn sender(&self) -> StoreResult<&mpsc::Sender<WriteCmd>> {
        self.write_tx
            .as_ref()
            .ok_or_else(|| GardenerError::Database("store is closed".to_string()))
    }

    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref().to_path_buf();
        append_run_log(
            "info",
            "backlog_store.open",
            json!({
                "path": path.display().to_string(),
                "path_state": backlog_path_state(&path),
            }),
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| GardenerError::Database(e.to_string()))?;
        }

        let existed_before_open = path.exists();
        if existed_before_open {
            let meta =
                std::fs::metadata(&path).map_err(|e| GardenerError::Database(e.to_string()))?;
            if meta.len() == 0 {
                append_run_log(
                    "error",
                    "backlog_store.open.zero_byte_rejected",
                    json!({
                        "path": path.display().to_string(),
                        "path_state": backlog_path_state(&path),
                    }),
                );
                return Err(GardenerError::Database(format!(
                    "backlog database is 0 bytes (corrupt): {}",
                    path.display()
                )));
            }
        }

        let mut write_conn = Connection::open(&path).map_err(db_err)?;
        configure_write_connection(&write_conn)?;

        if existed_before_open {
            let integrity: String = write_conn
                .pragma_query_value(None, "quick_check", |row| row.get(0))
                .map_err(db_err)?;
            if integrity != "ok" {
                append_run_log(
                    "error",
                    "backlog_store.integrity_check.failed",
                    json!({
                        "path": path.display().to_string(),
                        "result": integrity,
                    }),
                );
                return Err(GardenerError::Database(format!(
                    "backlog database failed integrity check: {integrity}"
                )));
            }
            append_run_log(
                "info",
                "backlog_store.integrity_check.passed",
                json!({ "path": path.display().to_string() }),
            );
        }

        run_migrations(&mut write_conn)?;

        let (write_tx, mut write_rx) = mpsc::channel(128);
        let writer_path = path.clone();
        let writer_join = thread::spawn(move || {
            while let Some(cmd) = write_rx.blocking_recv() {
                let (operation, command_payload) = write_cmd_details(&cmd);
                append_run_log(
                    "debug",
                    "backlog_store.write_command.received",
                    json!({
                        "operation": operation,
                        "command": command_payload,
                        "path": writer_path.display().to_string(),
                        "path_state": backlog_path_state(&writer_path),
                    }),
                );
                match cmd {
                    WriteCmd::Upsert { task, now, reply } => {
                        let result = upsert_task(&write_conn, &task, now).and_then(|_| {
                            fetch_task(&write_conn, &compute_task_id_from_new_task(&task))?
                                .ok_or_else(|| {
                                    GardenerError::Database("row missing after upsert".to_string())
                                })
                        });
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|task| {
                                json!({
                                    "task_id": task.task_id,
                                    "status": task.status.as_str(),
                                    "priority": task.priority.as_str(),
                                    "attempt_count": task.attempt_count,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::InsertManualTask { task, now, reply } => {
                        let result = insert_manual_task(&write_conn, &task, now).and_then(|_| {
                            fetch_task(&write_conn, &task.task_id)?.ok_or_else(|| {
                                GardenerError::Database(
                                    "row missing after manual insert".to_string(),
                                )
                            })
                        });
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|task| {
                                json!({
                                    "task_id": task.task_id,
                                    "status": task.status.as_str(),
                                    "priority": task.priority.as_str(),
                                    "attempt_count": task.attempt_count,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::ClaimNext {
                        lease_owner,
                        lease_expires_at,
                        now,
                        reply,
                    } => {
                        let result =
                            claim_next(&mut write_conn, &lease_owner, lease_expires_at, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|task| {
                                task.as_ref().map_or_else(
                                    || json!({ "claimed": false }),
                                    |claimed| {
                                        json!({
                                            "claimed": true,
                                            "task_id": claimed.task_id,
                                            "status": claimed.status.as_str(),
                                            "attempt_count": claimed.attempt_count,
                                        })
                                    },
                                )
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::MarkInProgress {
                        task_id,
                        lease_owner,
                        now,
                        reply,
                    } => {
                        let result = mark_in_progress(&write_conn, &task_id, &lease_owner, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "lease_owner": lease_owner,
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::MarkComplete {
                        task_id,
                        lease_owner,
                        now,
                        reply,
                    } => {
                        let result = mark_complete(&write_conn, &task_id, &lease_owner, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "lease_owner": lease_owner,
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::RecoverStale { now, reply } => {
                        let result = recover_stale(&write_conn, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result
                                .as_ref()
                                .map(|count| json!({ "recovered_count": count })),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::ReleaseLease {
                        task_id,
                        lease_owner,
                        now,
                        reply,
                    } => {
                        let result = release_lease(&write_conn, &task_id, &lease_owner, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "lease_owner": lease_owner,
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::MarkUnresolved {
                        task_id,
                        lease_owner,
                        now,
                        reply,
                    } => {
                        let result = mark_unresolved(&write_conn, &task_id, &lease_owner, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "lease_owner": lease_owner,
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::SetUnresolvedReady {
                        task_id,
                        now,
                        reply,
                    } => {
                        let result =
                            set_unresolved_status(&write_conn, &task_id, TaskStatus::Ready, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "new_status": TaskStatus::Ready.as_str(),
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::SetUnresolvedMergePending {
                        task_id,
                        now,
                        reply,
                    } => {
                        let result = set_unresolved_status(
                            &write_conn,
                            &task_id,
                            TaskStatus::MergePending,
                            now,
                        );
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "new_status": TaskStatus::MergePending.as_str(),
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::SetMergePendingReady {
                        task_id,
                        now,
                        reply,
                    } => {
                        let result = set_merge_pending_to_ready(&write_conn, &task_id, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "new_status": TaskStatus::Ready.as_str(),
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::ClearRelatedPr { task_id, reply } => {
                        let result = clear_related_pr(&write_conn, &task_id);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::MarkMergePending {
                        task_id,
                        lease_owner,
                        now,
                        reply,
                    } => {
                        let result = mark_merge_pending(&write_conn, &task_id, &lease_owner, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "lease_owner": lease_owner,
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::ClaimMergePending {
                        merge_worker_id,
                        now,
                        reply,
                    } => {
                        let result = claim_merge_pending(&write_conn, &merge_worker_id, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|task| {
                                task.as_ref().map_or_else(
                                    || json!({ "claimed": false }),
                                    |claimed| {
                                        json!({
                                            "claimed": true,
                                            "task_id": claimed.task_id,
                                            "status": claimed.status.as_str(),
                                        })
                                    },
                                )
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::SetRelatedPr {
                        task_id,
                        pr_number,
                        branch,
                        now,
                        reply,
                    } => {
                        let result = set_related_pr(&write_conn, &task_id, pr_number, &branch, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "pr_number": pr_number,
                                    "branch": branch,
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::PromoteReadyWithPr { now, reply } => {
                        let result = promote_ready_with_pr(&write_conn, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|count| json!({ "count": count })),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::ReopenCompleteToMergePending {
                        task_id,
                        now,
                        reply,
                    } => {
                        let result = reopen_complete_to_merge_pending(&write_conn, &task_id, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|changed| {
                                json!({
                                    "task_id": task_id,
                                    "changed": changed,
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::UpdateTaskMetadata {
                        task_id,
                        patch,
                        now,
                        reply,
                    } => {
                        let result = update_task_metadata(&write_conn, &task_id, &patch, now);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|mutation| {
                                json!({
                                    "task_id": mutation.after.task_id,
                                    "changed": mutation.changed,
                                    "before_status": mutation.before.status.as_str(),
                                    "after_status": mutation.after.status.as_str(),
                                })
                            }),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                    WriteCmd::InsertRejectedSeed {
                        title,
                        details,
                        rationale,
                        domain,
                        priority,
                        rejection_reason,
                        now,
                        reply,
                    } => {
                        let id = {
                            use sha2::{Digest, Sha256};
                            let mut hasher = Sha256::new();
                            hasher.update(title.trim().to_ascii_lowercase().as_bytes());
                            hasher.update(b"|");
                            hasher.update(domain.trim().to_ascii_lowercase().as_bytes());
                            let hash = format!("{:x}", hasher.finalize());
                            format!("rej-{}", &hash[..16])
                        };
                        let result = write_conn
                            .execute(
                                "INSERT OR REPLACE INTO rejected_seed_tasks \
                                 (id, title, details, rationale, domain, priority, rejection_reason, rejected_at) \
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                                params![id, title, details, rationale, domain, priority, rejection_reason, now],
                            )
                            .map(|_| ())
                            .map_err(db_err);
                        log_write_result(
                            &writer_path,
                            operation,
                            &result.as_ref().map(|_| json!({ "id": id, "title": title })),
                            result.as_ref().err(),
                        );
                        let _ = reply.send(result);
                    }
                }
            }
        });

        let read_pool = ReadPool::open(&path, READ_POOL_SIZE)?;
        let store = Self {
            write_tx: Some(write_tx),
            read_pool,
            writer_join: Some(writer_join),
            db_path: path.clone(),
        };

        let (p0, p1, p2) = store.count_tasks_by_priority().unwrap_or((0, 0, 0));
        append_run_log(
            "info",
            "backlog_store.opened",
            json!({
                "path": path.display().to_string(),
                "task_count": { "p0": p0, "p1": p1, "p2": p2, "total": p0 + p1 + p2 },
            }),
        );

        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

#[derive(Clone)]
pub(super) struct ReadPool {
    conns: Arc<Vec<Mutex<Connection>>>,
    next: Arc<AtomicUsize>,
}

impl ReadPool {
    fn open(path: &Path, size: usize) -> StoreResult<Self> {
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(db_err)?;
            conn.busy_timeout(std::time::Duration::from_secs(3))
                .map_err(db_err)?;
            conns.push(Mutex::new(conn));
        }

        Ok(Self {
            conns: Arc::new(conns),
            next: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> StoreResult<T>) -> StoreResult<T> {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        let guard = self.conns[idx]
            .lock()
            .map_err(|_| GardenerError::Database("read connection lock poisoned".to_string()))?;
        f(&guard)
    }
}

fn configure_write_connection(conn: &Connection) -> StoreResult<()> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(db_err)?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(db_err)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(db_err)?;
    Ok(())
}
