use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::priority::Priority;
use crate::task_identity::{compute_task_id, TaskIdentity, TaskKind};

const READ_POOL_SIZE: usize = 4;

type StoreResult<T> = Result<T, GardenerError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Ready,
    Leased,
    InProgress,
    Complete,
    Failed,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Leased => "leased",
            Self::InProgress => "in_progress",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }

    fn from_db(value: &str) -> Option<Self> {
        match value {
            "ready" => Some(Self::Ready),
            "leased" => Some(Self::Leased),
            "in_progress" => Some(Self::InProgress),
            "complete" => Some(Self::Complete),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklogTask {
    pub task_id: String,
    pub kind: TaskKind,
    pub title: String,
    pub details: String,
    pub rationale: String,
    pub scope_key: String,
    pub priority: Priority,
    pub status: TaskStatus,
    pub last_updated: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub source: String,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
    pub attempt_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    pub kind: TaskKind,
    pub title: String,
    pub details: String,
    pub rationale: String,
    pub scope_key: String,
    pub priority: Priority,
    pub source: String,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
}

#[derive(Debug)]
enum WriteCmd {
    Upsert {
        task: NewTask,
        now: i64,
        reply: oneshot::Sender<StoreResult<BacklogTask>>,
    },
    ClaimNext {
        lease_owner: String,
        lease_expires_at: i64,
        now: i64,
        reply: oneshot::Sender<StoreResult<Option<BacklogTask>>>,
    },
    MarkInProgress {
        task_id: String,
        lease_owner: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    MarkComplete {
        task_id: String,
        lease_owner: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    RecoverStale {
        now: i64,
        reply: oneshot::Sender<StoreResult<usize>>,
    },
    ReleaseLease {
        task_id: String,
        lease_owner: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
}

pub struct BacklogStore {
    write_tx: Option<mpsc::Sender<WriteCmd>>,
    read_pool: ReadPool,
    writer_join: Option<thread::JoinHandle<()>>,
    db_path: PathBuf,
}

impl Drop for BacklogStore {
    fn drop(&mut self) {
        // Close the sender first so the writer loop exits.
        drop(self.write_tx.take());
        // Then join the writer thread to flush any in-flight writes.
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
            json!({ "path": path.display().to_string() }),
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| GardenerError::Database(e.to_string()))?;
        }

        let existed_before_open = path.exists();

        // Reject zero-byte files — they indicate prior corruption.
        if existed_before_open {
            let meta = std::fs::metadata(&path)
                .map_err(|e| GardenerError::Database(e.to_string()))?;
            if meta.len() == 0 {
                return Err(GardenerError::Database(format!(
                    "backlog database is 0 bytes (corrupt): {}",
                    path.display()
                )));
            }
        }

        let mut write_conn = Connection::open(&path).map_err(db_err)?;
        configure_write_connection(&write_conn)?;

        // Run quick_check on existing databases to catch corruption early.
        if existed_before_open {
            let integrity: String = write_conn
                .pragma_query_value(None, "quick_check", |row| row.get(0))
                .map_err(db_err)?;
            if integrity != "ok" {
                return Err(GardenerError::Database(format!(
                    "backlog database failed integrity check: {integrity}"
                )));
            }
        }

        run_migrations(&mut write_conn)?;

        let (write_tx, mut write_rx) = mpsc::channel(128);
        let writer_join = thread::spawn(move || {
            while let Some(cmd) = write_rx.blocking_recv() {
                match cmd {
                    WriteCmd::Upsert { task, now, reply } => {
                        let result = upsert_task(&write_conn, &task, now).and_then(|_| {
                            fetch_task(&write_conn, &compute_task_id_from_new_task(&task))?
                                .ok_or_else(|| {
                                    GardenerError::Database(
                                        "row missing after upsert".to_string(),
                                    )
                                })
                        });
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
                        let _ = reply.send(result);
                    }
                    WriteCmd::MarkInProgress {
                        task_id,
                        lease_owner,
                        now,
                        reply,
                    } => {
                        let result = mark_in_progress(&write_conn, &task_id, &lease_owner, now);
                        let _ = reply.send(result);
                    }
                    WriteCmd::MarkComplete {
                        task_id,
                        lease_owner,
                        now,
                        reply,
                    } => {
                        let result = mark_complete(&write_conn, &task_id, &lease_owner, now);
                        let _ = reply.send(result);
                    }
                    WriteCmd::RecoverStale { now, reply } => {
                        let result = recover_stale(&write_conn, now);
                        let _ = reply.send(result);
                    }
                    WriteCmd::ReleaseLease {
                        task_id,
                        lease_owner,
                        now,
                        reply,
                    } => {
                        let result = release_lease(&write_conn, &task_id, &lease_owner, now);
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

        let recovered = store.recover_stale_leases(system_time_unix())?;
        append_run_log(
            "info",
            "backlog_store.opened",
            json!({
                "path": path.display().to_string(),
                "stale_recovered": recovered,
            }),
        );

        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

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
            Ok(_) => {}
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
                    WHERE status <> 'complete'"
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
        append_run_log(
            "debug",
            "backlog.tasks.count_active.started",
            json!({}),
        );
        self.read_pool.with_conn(|conn| {
            let mut statement = conn
                .prepare("SELECT COUNT(*) FROM backlog_tasks WHERE status NOT IN ('complete', 'failed')")
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
}

#[derive(Clone)]
struct ReadPool {
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

fn run_migrations(conn: &mut Connection) -> StoreResult<()> {
    let migrations = [
        (1_i64, include_str!("../migrations/0001_backlog.sql")),
        (2_i64, include_str!("../migrations/0002_backlog.sql")),
    ];

    conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL); COMMIT;")
        .map_err(db_err)?;

    for (version, sql) in migrations {
        let exists = conn
            .query_row(
                "SELECT 1 FROM schema_migrations WHERE version = ?1 LIMIT 1",
                [version],
                |_| Ok(()),
            )
            .optional()
            .map_err(db_err)?
            .is_some();

        if exists {
            continue;
        }

        append_run_log(
            "info",
            "backlog_store.migration.applying",
            json!({ "version": version }),
        );
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute_batch(sql).map_err(db_err)?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![version, system_time_unix()],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        append_run_log(
            "info",
            "backlog_store.migration.applied",
            json!({ "version": version }),
        );
    }

    Ok(())
}

fn upsert_task(conn: &Connection, task: &NewTask, now: i64) -> StoreResult<()> {
    append_run_log(
        "debug",
        "backlog_store.upsert_task.started",
        json!({
            "task_id": compute_task_id_from_new_task(task),
            "scope_key": task.scope_key,
        }),
    );
    let task_id = compute_task_id_from_new_task(task);
    conn.execute(
        "INSERT INTO backlog_tasks (
            task_id, kind, title, details, scope_key, priority, status, last_updated, lease_owner,
            lease_expires_at, source, related_pr, related_branch, rationale, attempt_count, created_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, 'ready', ?7, NULL, NULL, ?8, ?9, ?10, ?11, 0, ?12
        )
        ON CONFLICT(task_id) DO UPDATE SET
            title = excluded.title,
            details = excluded.details,
            scope_key = excluded.scope_key,
            priority = CASE
                WHEN CASE excluded.priority
                    WHEN 'P0' THEN 0
                    WHEN 'P1' THEN 1
                    ELSE 2
                END < CASE backlog_tasks.priority
                    WHEN 'P0' THEN 0
                    WHEN 'P1' THEN 1
                    ELSE 2
                END THEN excluded.priority
                ELSE backlog_tasks.priority
            END,
            status = CASE
                WHEN backlog_tasks.status IN ('leased', 'in_progress') THEN backlog_tasks.status
                ELSE 'ready'
            END,
            last_updated = excluded.last_updated,
            lease_owner = CASE
                WHEN backlog_tasks.status IN ('leased', 'in_progress') THEN backlog_tasks.lease_owner
                ELSE NULL
            END,
            lease_expires_at = CASE
                WHEN backlog_tasks.status IN ('leased', 'in_progress') THEN backlog_tasks.lease_expires_at
                ELSE NULL
            END,
            source = excluded.source,
            related_pr = excluded.related_pr,
            related_branch = excluded.related_branch,
            rationale = excluded.rationale",
        params![
            task_id,
            task.kind.as_str(),
            task.title,
            task.details,
            task.scope_key,
            task.priority.as_str(),
            now,
            task.source,
            task.related_pr,
            task.related_branch,
            task.rationale,
            now,
        ],
    )
    .map_err(db_err)?;
    Ok(())
}

fn claim_next(
    conn: &mut Connection,
    lease_owner: &str,
    lease_expires_at: i64,
    now: i64,
) -> StoreResult<Option<BacklogTask>> {
    let tx = conn.transaction().map_err(db_err)?;
    let maybe = claim_next_in_tx(&tx, lease_owner, lease_expires_at, now)?;
    tx.commit().map_err(db_err)?;
    Ok(maybe)
}

fn claim_next_in_tx(
    tx: &Transaction<'_>,
    lease_owner: &str,
    lease_expires_at: i64,
    now: i64,
) -> StoreResult<Option<BacklogTask>> {
    append_run_log(
        "debug",
        "backlog_store.claim_next_in_tx.started",
        json!({
            "lease_owner": lease_owner,
            "lease_expires_at": lease_expires_at
        }),
    );
    let mut candidate = tx
        .prepare(
            "SELECT task_id
             FROM backlog_tasks
             WHERE status = 'ready'
             ORDER BY
                CASE priority WHEN 'P0' THEN 0 WHEN 'P1' THEN 1 ELSE 2 END,
                CASE WHEN attempt_count > 0 THEN 0 ELSE 1 END,
                attempt_count DESC,
                last_updated ASC,
                created_at ASC
             LIMIT 1",
        )
        .map_err(db_err)?;
    let Some(task_id) = candidate
        .query_row([], |row| row.get::<_, String>(0))
        .optional()
        .map_err(db_err)?
    else {
        return Ok(None);
    };

    let mut stmt = tx
        .prepare(
            "UPDATE backlog_tasks
             SET status = 'leased',
                 lease_owner = ?2,
                 lease_expires_at = ?3,
                 last_updated = ?4,
                 attempt_count = attempt_count + 1
             WHERE task_id = ?1 AND status = 'ready'
             RETURNING task_id, kind, title, details, scope_key, priority, status, last_updated,
                       lease_owner, lease_expires_at, source, related_pr, related_branch, rationale,
                       attempt_count, created_at",
        )
        .map_err(db_err)?;

    stmt.query_row(
        params![task_id, lease_owner, lease_expires_at, now],
        row_to_task,
    )
    .optional()
    .map_err(db_err)
}

fn mark_in_progress(
    conn: &Connection,
    task_id: &str,
    lease_owner: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.mark_in_progress.started",
        json!({
            "task_id": task_id,
            "lease_owner": lease_owner,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'in_progress', last_updated = ?1
             WHERE task_id = ?2 AND status = 'leased' AND lease_owner = ?3",
            params![now, task_id, lease_owner],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

fn mark_complete(
    conn: &Connection,
    task_id: &str,
    lease_owner: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.mark_complete.started",
        json!({
            "task_id": task_id,
            "lease_owner": lease_owner,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'complete', lease_owner = NULL, lease_expires_at = NULL, last_updated = ?1
             WHERE task_id = ?2 AND lease_owner = ?3 AND status IN ('leased', 'in_progress')",
            params![now, task_id, lease_owner],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

fn release_lease(
    conn: &Connection,
    task_id: &str,
    lease_owner: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.release_lease.started",
        json!({
            "task_id": task_id,
            "lease_owner": lease_owner,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'ready', lease_owner = NULL, lease_expires_at = NULL, last_updated = ?1
             WHERE task_id = ?2 AND lease_owner = ?3 AND status IN ('leased', 'in_progress')",
            params![now, task_id, lease_owner],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

fn recover_stale(conn: &Connection, now: i64) -> StoreResult<usize> {
    append_run_log(
        "debug",
        "backlog_store.recover_stale.started",
        json!({ "now": now }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'ready',
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 last_updated = ?1
             WHERE status = 'in_progress'
                OR (status = 'leased' AND (lease_expires_at IS NULL OR lease_expires_at < ?1))",
            [now],
        )
        .map_err(db_err)?;
    Ok(changed)
}

fn fetch_task(conn: &Connection, task_id: &str) -> StoreResult<Option<BacklogTask>> {
    append_run_log(
        "debug",
        "backlog_store.fetch_task.started",
        json!({
            "task_id": task_id,
        }),
    );
    conn.query_row(
        "SELECT task_id, kind, title, details, scope_key, priority, status, last_updated,
                lease_owner, lease_expires_at, source, related_pr, related_branch, rationale,
                attempt_count, created_at
         FROM backlog_tasks
         WHERE task_id = ?1",
        [task_id],
        row_to_task,
    )
    .optional()
    .map_err(db_err)
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<BacklogTask> {
    let kind = row.get::<_, String>(1)?;
    let priority = row.get::<_, String>(5)?;
    let status = row.get::<_, String>(6)?;

    Ok(BacklogTask {
        task_id: row.get(0)?,
        kind: task_kind_from_db(&kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid kind",
                )),
            )
        })?,
        title: row.get(2)?,
        details: row.get(3)?,
        scope_key: row.get(4)?,
        rationale: row.get(13)?,
        priority: Priority::from_db(&priority).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid priority",
                )),
            )
        })?,
        status: TaskStatus::from_db(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid status",
                )),
            )
        })?,
        last_updated: row.get(7)?,
        lease_owner: row.get(8)?,
        lease_expires_at: row.get(9)?,
        source: row.get(10)?,
        related_pr: row.get(11)?,
        related_branch: row.get(12)?,
        attempt_count: row.get(14)?,
        created_at: row.get(15)?,
    })
}

fn task_kind_from_db(value: &str) -> Option<TaskKind> {
    match value {
        "quality_gap" => Some(TaskKind::QualityGap),
        "merge_conflict" => Some(TaskKind::MergeConflict),
        "pr_collision" => Some(TaskKind::PrCollision),
        "feature" => Some(TaskKind::Feature),
        "bugfix" => Some(TaskKind::Bugfix),
        "maintenance" => Some(TaskKind::Maintenance),
        "infra" => Some(TaskKind::Infra),
        _ => None,
    }
}

fn compute_task_id_from_new_task(task: &NewTask) -> String {
    compute_task_id(TaskIdentity {
        kind: task.kind,
        title: task.title.clone(),
        scope_key: task.scope_key.clone(),
        related_pr: task.related_pr,
        related_branch: task.related_branch.clone(),
    })
}

fn db_err(error: rusqlite::Error) -> GardenerError {
    GardenerError::Database(error.to_string())
}

pub fn system_time_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::{db_err, task_kind_from_db, BacklogStore, NewTask, TaskStatus};
    use crate::priority::Priority;
    use crate::task_identity::{compute_task_id, TaskIdentity, TaskKind};

    fn temp_store() -> (BacklogStore, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("backlog.sqlite");
        (BacklogStore::open(&db).expect("open store"), dir)
    }

    fn task(title: &str, priority: Priority) -> NewTask {
        NewTask {
            kind: TaskKind::Feature,
            title: title.to_string(),
            details: "details".to_string(),
            rationale: String::new(),
            scope_key: "domain:core".to_string(),
            priority,
            source: "test".to_string(),
            related_pr: None,
            related_branch: None,
        }
    }

    #[test]
    fn upsert_dedupes_and_upgrades_priority() {
        let (store, _dir) = temp_store();

        let first = store
            .upsert_task(task("Normalize scheduler order", Priority::P2))
            .expect("insert");
        let second = store
            .upsert_task(task("  normalize   scheduler order  ", Priority::P0))
            .expect("upsert");

        assert_eq!(first.task_id, second.task_id);
        assert_eq!(second.priority, Priority::P0);

        let tasks = store.list_tasks().expect("list");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].priority, Priority::P0);
    }

    #[test]
    fn lower_priority_reinsert_does_not_downgrade() {
        let (store, _dir) = temp_store();
        let _ = store
            .upsert_task(task("Fix lease collision", Priority::P0))
            .expect("insert");
        let row = store
            .upsert_task(task("fix lease collision", Priority::P2))
            .expect("upsert");
        assert_eq!(row.priority, Priority::P0);
    }

    #[test]
    fn claim_is_priority_ordered_fifo_by_last_updated() {
        let (store, _dir) = temp_store();
        let _ = store
            .upsert_task(task("task-1", Priority::P1))
            .expect("insert 1");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let _ = store
            .upsert_task(task("task-2", Priority::P0))
            .expect("insert 2");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let _ = store
            .upsert_task(task("task-3", Priority::P0))
            .expect("insert 3");

        let first = store
            .claim_next("worker-a", 60)
            .expect("claim")
            .expect("task");
        let second = store
            .claim_next("worker-b", 60)
            .expect("claim")
            .expect("task");
        let third = store
            .claim_next("worker-c", 60)
            .expect("claim")
            .expect("task");

        assert_eq!(first.title, "task-2");
        assert_eq!(second.title, "task-3");
        assert_eq!(third.title, "task-1");
    }

    #[test]
    fn claim_prioritizes_retries_within_same_priority() {
        let (store, _dir) = temp_store();
        let first = store
            .upsert_task(task("retry-me-first", Priority::P1))
            .expect("seed retry");

        let leased = store
            .claim_next("worker-a", 60)
            .expect("claim retry")
            .expect("retry row");
        assert_eq!(leased.task_id, first.task_id);
        let transitioned = store
            .mark_in_progress(&first.task_id, "worker-a")
            .expect("mark in progress");
        assert!(transitioned);
        let recovered = store.recover_stale_leases(i64::MAX).expect("recover stale");
        assert_eq!(recovered, 1);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let _second = store
            .upsert_task(task("fresh-task", Priority::P1))
            .expect("seed fresh");

        let claimed = store
            .claim_next("worker-b", 60)
            .expect("claim after retry")
            .expect("task");
        assert_eq!(claimed.task_id, first.task_id);
        assert_eq!(claimed.attempt_count, 2);
    }

    #[test]
    fn concurrent_claims_never_return_same_task() {
        let (store, _dir) = temp_store();
        let store = Arc::new(store);
        for idx in 0..25 {
            let _ = store
                .upsert_task(task(&format!("task-{idx}"), Priority::P1))
                .expect("seed task");
        }

        let mut joins = Vec::new();
        for worker in 0..25 {
            let store = Arc::clone(&store);
            joins.push(thread::spawn(move || {
                store
                    .claim_next(&format!("worker-{worker}"), 60)
                    .expect("claim")
                    .map(|task| task.task_id)
            }));
        }

        let mut claimed = Vec::new();
        for join in joins {
            if let Some(task_id) = join.join().expect("join") {
                claimed.push(task_id);
            }
        }

        let unique = claimed.iter().cloned().collect::<HashSet<_>>();
        assert_eq!(claimed.len(), unique.len());
        assert_eq!(claimed.len(), 25);
    }

    #[test]
    fn stale_recovery_requeues_in_progress_and_expired_leases() {
        let (store, _dir) = temp_store();
        let row = store
            .upsert_task(task("recover-me", Priority::P1))
            .expect("seed");

        let leased = store
            .claim_next("worker", 1)
            .expect("claim")
            .expect("leased row");
        assert_eq!(leased.status, TaskStatus::Leased);

        let transitioned = store
            .mark_in_progress(&row.task_id, "worker")
            .expect("in progress");
        assert!(transitioned);

        let recovered = store.recover_stale_leases(i64::MAX).expect("recover");
        assert_eq!(recovered, 1);

        let round_trip = store.get_task(&row.task_id).expect("fetch").expect("task");
        assert_eq!(round_trip.status, TaskStatus::Ready);
        assert_eq!(round_trip.lease_owner, None);
        assert_eq!(round_trip.lease_expires_at, None);
    }

    #[test]
    fn mark_complete_requires_owner_match() {
        let (store, _dir) = temp_store();
        let row = store
            .upsert_task(task("complete-me", Priority::P1))
            .expect("seed");
        let _ = store.claim_next("worker-a", 60).expect("claim");

        let denied = store
            .mark_complete(&row.task_id, "worker-b")
            .expect("mismatch");
        assert!(!denied);

        let allowed = store
            .mark_complete(&row.task_id, "worker-a")
            .expect("owner match");
        assert!(allowed);

        let task = store.get_task(&row.task_id).expect("fetch").expect("row");
        assert_eq!(task.status, TaskStatus::Complete);
    }

    #[test]
    fn task_identity_contract_matches_store_ids() {
        let input = task("Identity Task", Priority::P1);
        let expected = compute_task_id(TaskIdentity {
            kind: TaskKind::Feature,
            title: "identity task".to_string(),
            scope_key: "domain:core".to_string(),
            related_pr: None,
            related_branch: None,
        });

        let (store, _dir) = temp_store();
        let row = store.upsert_task(input).expect("insert");
        assert_eq!(row.task_id, expected);
    }

    #[test]
    fn covers_conversion_and_error_paths() {
        assert_eq!(TaskStatus::Ready.as_str(), "ready");
        assert_eq!(TaskStatus::Leased.as_str(), "leased");
        assert_eq!(TaskStatus::InProgress.as_str(), "in_progress");
        assert_eq!(TaskStatus::Complete.as_str(), "complete");
        assert_eq!(TaskStatus::Failed.as_str(), "failed");
        assert_eq!(TaskStatus::from_db("failed"), Some(TaskStatus::Failed));
        assert_eq!(TaskStatus::from_db("unknown"), None);

        let (store, _dir) = temp_store();
        assert!(store.db_path().ends_with("backlog.sqlite"));
        assert_eq!(task_kind_from_db("bugfix"), Some(TaskKind::Bugfix));
        assert_eq!(
            task_kind_from_db("maintenance"),
            Some(TaskKind::Maintenance)
        );
        assert_eq!(task_kind_from_db("infra"), Some(TaskKind::Infra));
        assert_eq!(task_kind_from_db("nope"), None);

        let _ = store
            .upsert_task(NewTask {
                kind: TaskKind::Bugfix,
                title: "b".to_string(),
                details: String::new(),
                rationale: String::new(),
                scope_key: "global".to_string(),
                priority: Priority::P1,
                source: "t".to_string(),
                related_pr: None,
                related_branch: None,
            })
            .expect("bugfix insert");
        let _ = store
            .upsert_task(NewTask {
                kind: TaskKind::Maintenance,
                title: "m".to_string(),
                details: String::new(),
                rationale: String::new(),
                scope_key: "global".to_string(),
                priority: Priority::P2,
                source: "t".to_string(),
                related_pr: None,
                related_branch: None,
            })
            .expect("maintenance insert");
        let _ = store
            .upsert_task(NewTask {
                kind: TaskKind::Infra,
                title: "i".to_string(),
                details: String::new(),
                rationale: String::new(),
                scope_key: "global".to_string(),
                priority: Priority::P0,
                source: "t".to_string(),
                related_pr: None,
                related_branch: None,
            })
            .expect("infra insert");

        // Re-open same path to hit the migration fast-path with existing version rows.
        let reopened = BacklogStore::open(store.db_path()).expect("reopen");
        assert!(reopened.list_tasks().expect("reopened list").len() >= 3);

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("gardener-backlog-file-parent-{nonce}"));
        std::fs::create_dir_all(&base).expect("mkdir");
        let parent_file = base.join("not-a-dir");
        std::fs::write(&parent_file, "x").expect("file");
        let invalid_db = parent_file.join("db.sqlite");
        let err = match BacklogStore::open(&invalid_db) {
            Ok(_) => panic!("parent should fail"),
            Err(err) => err,
        };
        assert!(matches!(err, crate::errors::GardenerError::Database(_)));

        let conversion_conn = Connection::open_in_memory().expect("open memory");
        let bad_kind = conversion_conn.query_row(
            "SELECT 'id', 'invalid', 'title', '', 'global', 'P1', 'ready', 1, NULL, NULL, 'src', NULL, NULL, '', 0, 1",
            [],
            super::row_to_task,
        );
        assert!(bad_kind.is_err());

        let bad_priority = conversion_conn.query_row(
            "SELECT 'id', 'feature', 'title', '', 'global', 'PX', 'ready', 1, NULL, NULL, 'src', NULL, NULL, '', 0, 1",
            [],
            super::row_to_task,
        );
        assert!(bad_priority.is_err());

        let bad_status = conversion_conn.query_row(
            "SELECT 'id', 'feature', 'title', '', 'global', 'P1', 'unknown', 1, NULL, NULL, 'src', NULL, NULL, '', 0, 1",
            [],
            super::row_to_task,
        );
        assert!(bad_status.is_err());

        let converted = db_err(rusqlite::Error::InvalidQuery);
        assert!(matches!(
            converted,
            crate::errors::GardenerError::Database(_)
        ));
    }

    #[test]
    fn count_tasks_by_priority_excludes_complete() {
        let (store, _dir) = temp_store();
        let ready_p1 = store
            .upsert_task(task("ready p1", Priority::P1))
            .expect("insert ready p1");
        let _ = store
            .upsert_task(task("ready p2", Priority::P2))
            .expect("insert ready p2");
        let complete = store
            .upsert_task(task("complete p0", Priority::P0))
            .expect("insert complete candidate");
        let claimed = store
            .claim_next("worker-1", 60)
            .expect("claim complete candidate")
            .expect("claimed task");
        assert_eq!(claimed.task_id, complete.task_id);
        let moved = store
            .mark_in_progress(&complete.task_id, "worker-1")
            .expect("mark in progress");
        assert!(moved);
        let completed = store
            .mark_complete(&complete.task_id, "worker-1")
            .expect("mark complete");
        assert!(completed);

        let _ = ready_p1;
        let (p0, p1, p2) = store.count_tasks_by_priority().expect("count");
        assert_eq!(p0, 0);
        assert_eq!(p1, 1);
        assert_eq!(p2, 1);
    }

    #[test]
    fn drop_flushes_pending_writes() {
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("backlog.sqlite");
        {
            let store = BacklogStore::open(&db).expect("open store");
            store
                .upsert_task(task("survive-drop", Priority::P1))
                .expect("upsert");
            // store is dropped here — Drop impl should flush the write
        }
        let reopened = BacklogStore::open(&db).expect("reopen");
        let tasks = reopened.list_tasks().expect("list");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "survive-drop");
    }

    #[test]
    fn open_rejects_zero_byte_file() {
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("backlog.sqlite");
        std::fs::write(&db, b"").expect("create zero-byte file");
        match BacklogStore::open(&db) {
            Err(crate::errors::GardenerError::Database(msg)) => {
                assert!(msg.contains("0 bytes"), "unexpected message: {msg}");
            }
            Err(e) => panic!("expected Database error, got: {e}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn open_rejects_corrupt_file() {
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("backlog.sqlite");
        std::fs::write(&db, b"this is not a sqlite database at all").expect("write garbage");
        match BacklogStore::open(&db) {
            Err(crate::errors::GardenerError::Database(_)) => {}
            Err(e) => panic!("expected Database error, got: {e}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }
}
