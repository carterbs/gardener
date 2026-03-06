use crate::backlog_store::{BacklogStore, TaskStatus};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::logging::{recent_worker_log_lines, structured_fallback_line};
use crate::runtime::Terminal;
use crate::tui::{BacklogView, QueueStats, WorkerRow};
use crate::types::RuntimeScope;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DashboardSnapshot {
    pub(super) stats: QueueStats,
    pub(super) backlog: BacklogView,
}

pub(super) fn dashboard_snapshot(store: &BacklogStore) -> Result<DashboardSnapshot, GardenerError> {
    let tasks = store.list_tasks()?;
    let mut stats = QueueStats {
        ready: 0,
        active: 0,
        failed: 0,
        unresolved: 0,
        merge_pending: 0,
        p0: 0,
        p1: 0,
        p2: 0,
    };
    let mut backlog = BacklogView::default();
    for task in tasks {
        match task.status {
            TaskStatus::Ready => stats.ready += 1,
            TaskStatus::Leased | TaskStatus::InProgress => {
                stats.active += 1;
                backlog.in_progress.push(format!(
                    "INP {} {} {}",
                    task.priority.as_str(),
                    short_task_id(&task.task_id),
                    task.title
                ));
            }
            TaskStatus::MergePending => {
                stats.merge_pending += 1;
                let task_title = task.title.clone();
                let merge_title = match task.related_pr.and_then(|pr| u64::try_from(pr).ok()) {
                    Some(pr_number) => format!("PR #{pr_number} {task_title}"),
                    None => task_title,
                };
                backlog.in_progress.push(format!(
                    "MRG {} {} {}",
                    task.priority.as_str(),
                    short_task_id(&task.task_id),
                    merge_title
                ));
            }
            TaskStatus::Failed => stats.failed += 1,
            TaskStatus::Unresolved => stats.unresolved += 1,
            TaskStatus::Complete => {}
        }
        if matches!(task.status, TaskStatus::Ready) {
            backlog.queued.push(format!(
                "Q {} {} {}",
                task.priority.as_str(),
                short_task_id(&task.task_id),
                task.title
            ));
        }
        match task.priority {
            crate::priority::Priority::P0 => stats.p0 += 1,
            crate::priority::Priority::P1 => stats.p1 += 1,
            crate::priority::Priority::P2 => stats.p2 += 1,
        }
    }
    Ok(DashboardSnapshot { stats, backlog })
}

pub(super) fn render(
    terminal: &dyn Terminal,
    workers: &[WorkerRow],
    snapshot: &DashboardSnapshot,
    heartbeat_interval_seconds: u64,
    lease_timeout_seconds: u64,
) -> Result<(), GardenerError> {
    if terminal.stdin_is_tty() {
        terminal.draw_dashboard_with_config(
            workers,
            &snapshot.stats,
            &snapshot.backlog,
            heartbeat_interval_seconds,
            lease_timeout_seconds,
        )?;
    } else {
        for row in workers {
            terminal.write_line(&structured_fallback_line(
                &row.worker_id,
                &row.state,
                &row.tool_line,
            ))?;
        }
    }
    Ok(())
}

pub(super) fn short_task_id(task_id: &str) -> &str {
    task_id.get(0..6).unwrap_or(task_id)
}

pub(super) fn worker_failure_prompt(worker_id: &str, task_id: &str, reason: &str) -> String {
    let recent_lines = recent_worker_log_lines(worker_id, 15);
    let recent_log_summary = if recent_lines.is_empty() {
        "No recent worker log lines were available.".to_string()
    } else {
        recent_lines.join("\n")
    };
    format!(
        "Worker failure on task {task_id}\n\nError:\n{reason}\n\nLast 15 logs for {worker_id}:\n{recent_log_summary}\n\nPrompt to pass to an agent:\nInvestigate this failure, identify the exact root cause, and provide a remediation step-by-step.\nUse the context above, especially the jsonl worker logs, as the primary evidence."
    )
}

pub(super) fn quality_report_path(cfg: &AppConfig, scope: &RuntimeScope) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(&cfg.quality_report.path);
    if path.is_absolute() {
        path
    } else {
        scope.working_dir.join(path)
    }
}
