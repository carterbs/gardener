use rusqlite::{Connection, OptionalExtension};
use serde_json::json;

use crate::logging::append_run_log;
use crate::priority::Priority;
use crate::task_identity::TaskKind;

use super::logging::db_err;
use super::types::{BacklogTask, TaskStatus};
use super::types::StoreResult;

pub(super) fn fetch_task(conn: &Connection, task_id: &str) -> StoreResult<Option<BacklogTask>> {
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

pub(super) fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<BacklogTask> {
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

pub(super) fn task_kind_from_db(value: &str) -> Option<TaskKind> {
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
