use super::scheduling::append_worker_command;
use super::{DoingSummaryHandling, MergeSummaryHandling, MERGE_WORKER_ID};
use crate::backlog_store::BacklogStore;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::tui::WorkerRow;
use crate::worker::WorkerRunSummary;
use serde_json::json;

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_doing_complete_transition(
    store: &BacklogStore,
    workers: &mut [WorkerRow],
    worker_idx: usize,
    last_worker_state_line: usize,
    worker_id: &str,
    task_id: &str,
    completed: &mut usize,
    failed: &mut usize,
) -> Result<DoingSummaryHandling, GardenerError> {
    let marked_complete = store.mark_complete(task_id, worker_id)?;
    if !marked_complete {
        append_run_log(
            "error",
            "worker.task.completed_rejected",
            json!({
                "worker_id": worker_id,
                "task_id": task_id,
            }),
        );
        let unresolved = store.mark_unresolved(task_id, worker_id)?;
        *failed = failed.saturating_add(1);
        workers[worker_idx].state = if unresolved {
            "unresolved".to_string()
        } else {
            "failed".to_string()
        };
        workers[worker_idx].task_id = None;
        workers[worker_idx].last_state_line = last_worker_state_line;
        let message = if unresolved {
            format!("unresolved {}", task_id)
        } else {
            format!("complete transition rejected {}", task_id)
        };
        workers[worker_idx].tool_line = message.clone();
        append_worker_command(&mut workers[worker_idx], &message);
        workers[worker_idx].breadcrumb = workers[worker_idx].state.clone();
        workers[worker_idx].lease_held = false;
        return Ok(DoingSummaryHandling::ContinueLoop);
    }
    *completed = completed.saturating_add(1);
    workers[worker_idx].state = "complete".to_string();
    workers[worker_idx].task_id = None;
    workers[worker_idx].last_state_line = last_worker_state_line;
    let completed_message = format!("completed {}", task_id);
    workers[worker_idx].tool_line = completed_message.clone();
    append_worker_command(&mut workers[worker_idx], &completed_message);
    workers[worker_idx].breadcrumb = "complete".to_string();
    workers[worker_idx].lease_held = false;
    append_run_log(
        "info",
        "worker.task.completed",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "completed": *completed
        }),
    );
    Ok(DoingSummaryHandling::ContinueLoop)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_doing_non_complete_transition(
    store: &BacklogStore,
    workers: &mut [WorkerRow],
    worker_idx: usize,
    last_worker_state_line: usize,
    worker_id: &str,
    task_id: &str,
    summary: &WorkerRunSummary,
    failed: &mut usize,
) -> Result<DoingSummaryHandling, GardenerError> {
    workers[worker_idx].state = "failed".to_string();
    workers[worker_idx].task_id = None;
    workers[worker_idx].last_state_line = last_worker_state_line;
    let failed_message = if let Some(reason) = summary.failure_reason.clone() {
        if reason.is_empty() {
            format!("failed {}", task_id)
        } else {
            let truncated = reason.chars().take(150).collect::<String>();
            if reason.chars().count() > 150 {
                format!("failed: {}…", truncated)
            } else {
                format!("failed: {}", reason)
            }
        }
    } else {
        format!("failed {}", task_id)
    };
    workers[worker_idx].tool_line = failed_message.clone();
    append_worker_command(&mut workers[worker_idx], &failed_message);
    workers[worker_idx].breadcrumb = "failed".to_string();
    workers[worker_idx].lease_held = false;
    *failed = failed.saturating_add(1);
    append_run_log(
        "error",
        "worker.task.failed",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "final_state": summary.final_state.as_str()
        }),
    );

    let unresolved = store.mark_unresolved(task_id, worker_id)?;
    append_run_log(
        "warn",
        "worker.task.unresolved",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "marked_unresolved": unresolved,
            "failure_reason": summary.failure_reason,
            "final_state": summary.final_state.as_str(),
        }),
    );
    if unresolved {
        let unresolved_message = format!("unresolved {}", task_id);
        workers[worker_idx].state = "unresolved".to_string();
        workers[worker_idx].last_state_line = last_worker_state_line;
        workers[worker_idx].tool_line = unresolved_message.clone();
        workers[worker_idx].breadcrumb = "unresolved".to_string();
        append_worker_command(&mut workers[worker_idx], &unresolved_message);
    }
    Ok(DoingSummaryHandling::ContinueLoop)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_merge_summary(
    store: &BacklogStore,
    workers: &mut [WorkerRow],
    merge_row_idx: usize,
    last_worker_state_line: usize,
    task_id: &str,
    summary: &WorkerRunSummary,
    completed: &mut usize,
    merged: &mut usize,
    failed: &mut usize,
) -> Result<MergeSummaryHandling, GardenerError> {
    if summary.final_state == crate::types::WorkerState::Complete {
        let marked_complete = store.mark_complete(task_id, MERGE_WORKER_ID)?;
        if !marked_complete {
            append_run_log(
                "error",
                "merge_worker.task.completed_rejected",
                json!({
                    "worker_id": MERGE_WORKER_ID,
                    "task_id": task_id,
                }),
            );
            let _ = store.mark_unresolved(task_id, MERGE_WORKER_ID);
            *failed = failed.saturating_add(1);
            workers[merge_row_idx].state = "failed".to_string();
            workers[merge_row_idx].task_id = Some(task_id.to_string());
            workers[merge_row_idx].last_state_line = last_worker_state_line;
            let fail_msg = format!("merge complete transition rejected {}", task_id);
            workers[merge_row_idx].tool_line = fail_msg.clone();
            append_worker_command(&mut workers[merge_row_idx], &fail_msg);
            workers[merge_row_idx].breadcrumb = "failed".to_string();
            workers[merge_row_idx].lease_held = false;
            return Ok(MergeSummaryHandling::EarlyContinue);
        }
        *completed = completed.saturating_add(1);
        *merged = merged.saturating_add(1);
        workers[merge_row_idx].state = "complete".to_string();
        workers[merge_row_idx].task_id = Some(task_id.to_string());
        workers[merge_row_idx].last_state_line = last_worker_state_line;
        let done_msg = format!("merged {}", task_id);
        workers[merge_row_idx].tool_line = done_msg.clone();
        append_worker_command(&mut workers[merge_row_idx], &done_msg);
        workers[merge_row_idx].breadcrumb = "complete".to_string();
        workers[merge_row_idx].lease_held = false;
        append_run_log(
            "info",
            "merge_worker.task.completed",
            json!({
                "worker_id": MERGE_WORKER_ID,
                "task_id": task_id,
                "completed": *completed
            }),
        );
    } else {
        let _ = store.mark_unresolved(task_id, MERGE_WORKER_ID)?;
        *failed = failed.saturating_add(1);
        workers[merge_row_idx].state = "failed".to_string();
        workers[merge_row_idx].task_id = Some(task_id.to_string());
        workers[merge_row_idx].last_state_line = last_worker_state_line;
        let fail_msg = summary.failure_reason.as_deref().unwrap_or("merge failed");
        let truncated = fail_msg.chars().take(100).collect::<String>();
        workers[merge_row_idx].tool_line = truncated.clone();
        append_worker_command(&mut workers[merge_row_idx], &truncated);
        workers[merge_row_idx].breadcrumb = "failed".to_string();
        workers[merge_row_idx].lease_held = false;
    }
    Ok(MergeSummaryHandling::ContinueLoop)
}
