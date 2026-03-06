use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::backlog_store::{BacklogTask, TaskMutation, TaskStatus, TaskUpdatePatch};
use crate::errors::GardenerError;
use crate::logging::structured_fallback_line;
use crate::priority::Priority;
use crate::task_identity::TaskKind;

const PRIORITY_VALUES: &str = "P0|P1|P2";
const STATUS_VALUES: &str = "ready|leased|in_progress|merge_pending|complete|failed|unresolved";
const KIND_VALUES: &str =
    "feature|maintenance|quality_gap|bugfix|infra|merge_conflict|pr_collision";

pub fn resolve_db_path(custom: Option<PathBuf>) -> Result<PathBuf, GardenerError> {
    let _ = structured_fallback_line("backlog_db", "resolve_db_path", "resolving");
    if let Some(path) = custom {
        return Ok(path);
    }

    if let Some(path) = std::env::var_os("GARDENER_DB_PATH") {
        return Ok(PathBuf::from(path));
    }

    let home = std::env::var_os("HOME")
        .ok_or_else(|| GardenerError::Cli("HOME is not set; pass --db explicitly".to_string()))?;
    Ok(PathBuf::from(home).join(".gardener/backlog.sqlite"))
}

pub fn ensure_db_exists(path: &Path) -> Result<(), GardenerError> {
    let _ = structured_fallback_line("backlog_db", "ensure_db_exists", "checking");
    if path.is_file() {
        Ok(())
    } else {
        Err(GardenerError::Cli(format!(
            "database file not found: {}",
            path.display()
        )))
    }
}

pub fn runbook_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("runbooks")
        .join("backlog-operations.md")
}

pub fn default_manual_task_id(scope_key: &str, now: i64) -> String {
    format!("manual:{scope_key}:auto-{now}")
}

pub fn parse_priority(raw: &str) -> Result<Priority, GardenerError> {
    match raw {
        "P0" => Ok(Priority::P0),
        "P1" => Ok(Priority::P1),
        "P2" => Ok(Priority::P2),
        _ => Err(GardenerError::Cli(format!(
            "invalid --priority: {raw} (expected one of: {PRIORITY_VALUES})"
        ))),
    }
}

pub fn parse_status(raw: &str) -> Result<TaskStatus, GardenerError> {
    match raw {
        "ready" => Ok(TaskStatus::Ready),
        "leased" => Ok(TaskStatus::Leased),
        "in_progress" => Ok(TaskStatus::InProgress),
        "merge_pending" => Ok(TaskStatus::MergePending),
        "complete" => Ok(TaskStatus::Complete),
        "failed" => Ok(TaskStatus::Failed),
        "unresolved" => Ok(TaskStatus::Unresolved),
        _ => Err(GardenerError::Cli(format!(
            "invalid --status: {raw} (expected one of: {STATUS_VALUES})"
        ))),
    }
}

pub fn parse_kind(raw: &str) -> Result<TaskKind, GardenerError> {
    match raw {
        "feature" => Ok(TaskKind::Feature),
        "maintenance" => Ok(TaskKind::Maintenance),
        "quality_gap" => Ok(TaskKind::QualityGap),
        "bugfix" => Ok(TaskKind::Bugfix),
        "infra" => Ok(TaskKind::Infra),
        "merge_conflict" => Ok(TaskKind::MergeConflict),
        "pr_collision" => Ok(TaskKind::PrCollision),
        _ => Err(GardenerError::Cli(format!(
            "invalid --kind: {raw} (expected one of: {KIND_VALUES})"
        ))),
    }
}

pub fn validate_update_status(
    _before: &BacklogTask,
    status: TaskStatus,
) -> Result<(), GardenerError> {
    if matches!(status, TaskStatus::Leased | TaskStatus::InProgress) {
        return Err(GardenerError::Cli(
            "manual update does not support status leased or in_progress".to_string(),
        ));
    }

    Ok(())
}

pub fn validate_retire_status(status: TaskStatus) -> Result<(), GardenerError> {
    let _ = structured_fallback_line("backlog_db", "validate_retire_status", "validating");
    if matches!(status, TaskStatus::Complete | TaskStatus::Failed) {
        Ok(())
    } else {
        Err(GardenerError::Cli(
            "retire requires --status complete or --status failed".to_string(),
        ))
    }
}

pub fn format_list_row(task: &BacklogTask) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        task.task_id,
        task.title,
        task.priority.as_str(),
        task.status.as_str(),
        task.source,
        task.scope_key
    )
}

pub fn format_task(task: &BacklogTask) -> String {
    [
        format!("task_id: {}", task.task_id),
        format!("kind: {}", task.kind.as_str()),
        format!("title: {}", task.title),
        format!("details: {}", task.details),
        format!("rationale: {}", task.rationale),
        format!("scope_key: {}", task.scope_key),
        format!("priority: {}", task.priority.as_str()),
        format!("status: {}", task.status.as_str()),
        format!("lease_owner: {}", task.lease_owner.as_deref().unwrap_or("")),
        format!(
            "lease_expires_at: {}",
            task.lease_expires_at
                .map_or_else(String::new, |value| value.to_string())
        ),
        format!("source: {}", task.source),
        format!(
            "related_pr: {}",
            task.related_pr
                .map_or_else(String::new, |value| value.to_string())
        ),
        format!(
            "related_branch: {}",
            task.related_branch.as_deref().unwrap_or("")
        ),
        format!("attempt_count: {}", task.attempt_count),
        format!("created_at: {}", task.created_at),
        format!("last_updated: {}", task.last_updated),
    ]
    .join("\n")
}

#[derive(Debug, Serialize)]
pub struct TaskJson {
    pub task_id: String,
    pub kind: String,
    pub title: String,
    pub details: String,
    pub rationale: String,
    pub scope_key: String,
    pub priority: String,
    pub status: String,
    pub last_updated: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub source: String,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
    pub attempt_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct MutationJson {
    pub before: TaskJson,
    pub after: TaskJson,
    pub changed: bool,
}

impl From<&BacklogTask> for TaskJson {
    fn from(task: &BacklogTask) -> Self {
        Self {
            task_id: task.task_id.clone(),
            kind: task.kind.as_str().to_string(),
            title: task.title.clone(),
            details: task.details.clone(),
            rationale: task.rationale.clone(),
            scope_key: task.scope_key.clone(),
            priority: task.priority.as_str().to_string(),
            status: task.status.as_str().to_string(),
            last_updated: task.last_updated,
            lease_owner: task.lease_owner.clone(),
            lease_expires_at: task.lease_expires_at,
            source: task.source.clone(),
            related_pr: task.related_pr,
            related_branch: task.related_branch.clone(),
            attempt_count: task.attempt_count,
            created_at: task.created_at,
        }
    }
}

impl From<&TaskMutation> for MutationJson {
    fn from(mutation: &TaskMutation) -> Self {
        Self {
            before: TaskJson::from(&mutation.before),
            after: TaskJson::from(&mutation.after),
            changed: mutation.changed,
        }
    }
}

pub fn render_json<T: Serialize>(value: &T) -> Result<String, GardenerError> {
    serde_json::to_string_pretty(value)
        .map_err(|error| GardenerError::Cli(format!("failed to serialize JSON output: {error}")))
}

pub fn patch_has_changes(patch: &TaskUpdatePatch) -> bool {
    patch.status.is_some()
        || patch.rationale.is_some()
        || patch.related_pr.is_some()
        || patch.related_branch.is_some()
        || patch.clear_lease
}
