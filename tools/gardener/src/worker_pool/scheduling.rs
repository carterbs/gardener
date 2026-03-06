use super::dashboard::{dashboard_snapshot, render};
use super::util::now_hhmmss;
use super::{available_doing_slots, MERGE_WORKER_ID, WORKER_COMMAND_HISTORY_LIMIT};
use crate::backlog_store::{BacklogStore, BacklogTask};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::runtime::Terminal;
use crate::tui::WorkerRow;
use crate::worker::{worktree_branch_for, worktree_path_for, MergeRequest};
use crate::worker_identity::WorkerIdentity;
use crate::worktree::WorktreeClient;
use serde_json::json;
use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

pub(super) fn claim_tasks_for_available_workers(
    workers: &mut [WorkerRow],
    claimed: &mut Vec<(usize, BacklogTask)>,
    completed: usize,
    active_merging: usize,
    last_worker_state_line: usize,
    last_activity_pulse: &mut [Instant],
    parallelism: usize,
    target: usize,
    run_started_at_ms: i64,
    store: &BacklogStore,
    cfg: &AppConfig,
    terminal: &dyn Terminal,
    hb: u64,
    lt: u64,
) -> Result<bool, GardenerError> {
    let mut claimed_any = false;
    claimed.clear();
    let available_slots = available_doing_slots(parallelism, target, completed, active_merging);
    for idx in 0..available_slots {
        let worker_id = workers[idx].worker_id.clone();
        let claimed_task =
            store.claim_next(&worker_id, cfg.scheduler.lease_timeout_seconds as i64)?;
        let Some(task) = claimed_task else {
            set_worker_idle(&mut workers[idx], "waiting for claim");
            continue;
        };
        claimed_any = true;
        let task_age_ms = run_started_at_ms.saturating_sub(task.created_at);
        let inserted_after_run_start = task.created_at >= run_started_at_ms;
        append_run_log(
            "info",
            "worker.task.claimed",
            json!({
                "worker_id": worker_id,
                "task_id": task.task_id,
                "title": task.title,
                "task_created_at": task.created_at,
                "task_last_updated": task.last_updated,
                "run_started_at_ms": run_started_at_ms,
                "task_age_ms": task_age_ms,
                "inserted_after_run_start": inserted_after_run_start
            }),
        );
        let moved_to_in_progress = store.mark_in_progress(&task.task_id, &worker_id)?;
        if !moved_to_in_progress {
            append_run_log(
                "error",
                "worker.task.claim_transition_rejected",
                json!({
                    "worker_id": worker_id,
                    "task_id": task.task_id,
                    "transition": "mark_in_progress",
                }),
            );
            continue;
        }
        workers[idx].state = "claimed".to_string();
        workers[idx].task_title = task.title.clone();
        workers[idx].tool_line = "claimed".to_string();
        workers[idx].task_id = Some(task.task_id.clone());
        workers[idx].last_state_line = last_worker_state_line;
        workers[idx].breadcrumb = "claim>claimed".to_string();
        workers[idx].lease_held = true;
        append_worker_command(&mut workers[idx], "claimed");
        refresh_worker_heartbeats(workers, last_activity_pulse);
        render(terminal, workers, &dashboard_snapshot(store)?, hb, lt)?;
        claimed.push((idx, task));
    }
    Ok(claimed_any)
}

pub(super) fn mark_merge_worker_busy(
    workers: &mut [WorkerRow],
    last_activity_pulse: &mut [Instant],
    merge_row_idx: usize,
    pr_number: u64,
    task_id: &str,
    last_worker_state_line: usize,
    task_summary: &str,
) {
    workers[merge_row_idx].state = "merging".to_string();
    workers[merge_row_idx].task_id = Some(task_id.to_string());
    workers[merge_row_idx].last_state_line = last_worker_state_line;
    workers[merge_row_idx].task_title = format!("PR #{pr_number} {task_summary}");
    let merge_msg = format!("merging PR #{pr_number}");
    workers[merge_row_idx].tool_line = merge_msg.clone();
    append_worker_command(&mut workers[merge_row_idx], &merge_msg);
    workers[merge_row_idx].breadcrumb = "merging".to_string();
    workers[merge_row_idx].lease_held = true;
    last_activity_pulse[merge_row_idx] = Instant::now();
}

pub(super) fn maybe_start_merge(
    active_merging: &mut usize,
    merge_tx: &mut Option<mpsc::Sender<MergeRequest>>,
    store: &BacklogStore,
    repo_root: &Path,
    worktree_client: &WorktreeClient<'_>,
) -> Result<Option<(u64, String, String)>, GardenerError> {
    if *active_merging >= 1 {
        return Ok(None);
    }
    loop {
        let Some(task) = store.claim_merge_pending(MERGE_WORKER_ID)? else {
            return Ok(None);
        };
        let task_id = task.task_id.clone();
        let pr_number = match task.related_pr.and_then(|n| u64::try_from(n).ok()) {
            Some(pr) => pr,
            None => {
                append_run_log(
                    "warn",
                    "worker_pool.merge_preseed.invalid_pr",
                    json!({
                        "task_id": task_id,
                        "worker_id": MERGE_WORKER_ID,
                    }),
                );
                let demoted = store.release_lease(&task.task_id, MERGE_WORKER_ID)?;
                if !demoted {
                    append_run_log(
                        "error",
                        "worker_pool.merge_preseed.invalid_pr_requeue_failed",
                        json!({
                            "task_id": task_id,
                            "worker_id": MERGE_WORKER_ID,
                        }),
                    );
                    return Ok(None);
                }
                continue;
            }
        };
        let branch = task
            .related_branch
            .clone()
            .unwrap_or_else(|| worktree_branch_for(&task.task_id));
        let worktree_path = worktree_path_for(repo_root, &task.task_id);
        if let Err(error) = worktree_client.create_or_resume(&worktree_path, &branch) {
            append_run_log(
                "warn",
                "worker_pool.merge_preseed.worktree_failed",
                json!({
                    "task_id": task_id,
                    "branch": branch,
                    "error": error.to_string(),
                    "worker_id": MERGE_WORKER_ID,
                }),
            );
            let requeued = store.mark_merge_pending(&task.task_id, MERGE_WORKER_ID)?;
            if !requeued {
                append_run_log(
                    "error",
                    "worker_pool.merge_preseed.requeue_failed",
                    json!({
                        "task_id": task_id,
                        "worker_id": MERGE_WORKER_ID,
                    }),
                );
            }
            return Ok(None);
        }
        if let Some(mtx) = merge_tx {
            let identity = WorkerIdentity::new(MERGE_WORKER_ID);
            let task_summary = task.title;
            let merge_request = MergeRequest {
                slot_idx: 0,
                task_id: task.task_id.clone(),
                task_summary: task_summary.clone(),
                attempt_count: task.attempt_count,
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                worktree_path,
                branch,
                pr_number,
                logs: Vec::new(),
                handoff_evidence_bundle: None,
            };
            if mtx.send(merge_request).is_err() {
                append_run_log(
                    "error",
                    "worker_pool.merge_preseed.dispatch_failed",
                    json!({
                        "task_id": task_id,
                        "worker_id": MERGE_WORKER_ID,
                    }),
                );
                let requeued = store.mark_merge_pending(&task.task_id, MERGE_WORKER_ID)?;
                if !requeued {
                    append_run_log(
                        "error",
                        "worker_pool.merge_preseed.dispatch_requeue_failed",
                        json!({
                            "task_id": task_id,
                            "worker_id": MERGE_WORKER_ID,
                        }),
                    );
                }
                return Ok(None);
            }
            *active_merging = active_merging.saturating_add(1);
            return Ok(Some((pr_number, task_id, task_summary)));
        }
        append_run_log(
            "warn",
            "worker_pool.merge_preseed.channel_closed",
            json!({
                "task_id": task_id,
                "worker_id": MERGE_WORKER_ID,
            }),
        );
        let requeued = store.mark_merge_pending(&task.task_id, MERGE_WORKER_ID)?;
        if !requeued {
            append_run_log(
                "error",
                "worker_pool.merge_preseed.channel_closed_requeue_failed",
                json!({
                    "task_id": task_id,
                    "worker_id": MERGE_WORKER_ID,
                }),
            );
        }
        return Ok(None);
    }
}

pub(super) fn append_worker_command(worker: &mut WorkerRow, command: &str) {
    worker
        .command_details
        .push((now_hhmmss(), command.to_string()));
    if worker.command_details.len() > WORKER_COMMAND_HISTORY_LIMIT {
        let overflow = worker.command_details.len() - WORKER_COMMAND_HISTORY_LIMIT;
        worker.command_details.drain(0..overflow);
    }
}

pub(super) fn execution_task_packet(task: &BacklogTask, task_override: Option<&str>) -> String {
    if let Some(task_override) = task_override {
        return task_override.to_string();
    }
    let details = task.details.trim();
    if details.is_empty() {
        task.title.clone()
    } else {
        format!("{}\n\n{}", task.title, details)
    }
}

pub(super) fn set_worker_idle(worker: &mut WorkerRow, tool_line: &str) {
    let should_log = worker.state != "idle" || worker.tool_line != tool_line;
    worker.state = "idle".to_string();
    worker.task_title = "idle".to_string();
    worker.tool_line = tool_line.to_string();
    worker.breadcrumb = "idle".to_string();
    worker.lease_held = false;
    worker.command_details.clear();
    if should_log {
        append_worker_command(worker, tool_line);
    }
}

pub(super) fn refresh_worker_heartbeats(
    workers: &mut [WorkerRow],
    last_activity_pulse: &[Instant],
) {
    let now = Instant::now();
    for (idx, worker) in workers.iter_mut().enumerate() {
        if let Some(last_pulse) = last_activity_pulse.get(idx) {
            let age = now.duration_since(*last_pulse).as_secs();
            worker.last_heartbeat_secs = age;
            worker.session_age_secs = age;
        }
    }
}
