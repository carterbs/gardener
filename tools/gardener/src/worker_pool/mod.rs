use crate::backlog_store::BacklogStore;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::hotkeys::operator_hotkeys_enabled;
use crate::logging::append_run_log;
use crate::runtime::Terminal;
use crate::runtime::{clear_interrupt, ProductionRuntime};
use crate::tui::{reset_workers_scroll, WorkerRow};
use crate::types::RuntimeScope;
use crate::worker::WorkerOutcome;
use crate::worktree::WorktreeClient;
use serde_json::json;
use std::time::Instant;

mod dashboard;
mod event_handling;
mod hotkeys;
mod result_handling;
mod scheduling;
mod util;

use dashboard::{dashboard_snapshot, render, worker_failure_prompt};
use hotkeys::{handle_hotkeys, wait_for_quit, HotkeyState};
use result_handling::run_pool_epoch;
use scheduling::{refresh_worker_heartbeats, set_worker_idle};
use util::{now_unix_millis, ShutdownSummary};

const WORKER_POOL_ID: &str = "worker_pool";
const MERGE_WORKER_ID: &str = "merge-worker";
const WORKER_COMMAND_HISTORY_LIMIT: usize = 20;
const COPY_SHORTCUT_KEY: char = 'c';
const IDLE_WORKER_POLL_DELAY_MS: u64 = 250;
const IDLE_WORKER_POLL_ATTEMPTS: usize = 4;

/// Messages on the result channel. Doing workers send `DoingResult`, the merge
/// worker sends `MergeResult`.
pub(super) enum PoolResultMessage {
    DoingResult {
        slot_idx: usize,
        task_id: String,
        result: Result<WorkerOutcome, GardenerError>,
    },
    MergeResult {
        task_id: String,
        result: Result<crate::worker::WorkerRunSummary, GardenerError>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PoolStreamEvent {
    ToolCommand {
        slot_idx: usize,
        task_id: String,
        command: String,
    },
    StateChanged {
        slot_idx: usize,
        task_id: String,
        state: String,
        details: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeSummaryHandling {
    ContinueLoop,
    EarlyContinue,
}

pub(super) fn available_doing_slots(
    parallelism: usize,
    target: usize,
    completed: usize,
    active_merging: usize,
) -> usize {
    let in_flight_completed_or_merging = completed.saturating_add(active_merging);
    parallelism.min(target.saturating_sub(in_flight_completed_or_merging))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoingSummaryHandling {
    ContinueLoop,
}

pub fn run_worker_pool_fsm(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    store: &BacklogStore,
    terminal: &dyn Terminal,
    target: usize,
    task_override: Option<&str>,
) -> Result<usize, GardenerError> {
    clear_interrupt();
    reset_workers_scroll();
    append_run_log(
        "info",
        "worker_pool.started",
        json!({
            "worker_id": WORKER_POOL_ID,
            "target": target,
            "configured_parallelism": cfg.orchestrator.parallelism,
            "task_override": task_override
        }),
    );
    let operator_hotkeys = operator_hotkeys_enabled();
    let mut report_visible = false;
    let mut report_content: Option<String> = None;
    let hb = cfg.scheduler.heartbeat_interval_seconds;
    let lt = cfg.scheduler.lease_timeout_seconds;
    let parallelism = cfg.orchestrator.parallelism.max(1) as usize;
    let mut workers = (0..parallelism)
        .map(|idx| WorkerRow {
            worker_id: format!("worker-{}", idx + 1),
            state: "idle".to_string(),
            task_id: None,
            last_state_line: 0,
            task_title: "idle".to_string(),
            tool_line: "waiting for claim".to_string(),
            breadcrumb: "idle".to_string(),
            last_heartbeat_secs: 0,
            session_age_secs: 0,
            lease_held: false,
            session_missing: false,
            command_details: Vec::new(),
        })
        .map(|mut worker| {
            set_worker_idle(&mut worker, "waiting for claim");
            worker
        })
        .collect::<Vec<_>>();
    // The merge worker TUI row is always the last row.
    let merge_row_idx = workers.len();
    workers.push({
        let mut row = WorkerRow {
            worker_id: MERGE_WORKER_ID.to_string(),
            state: "idle".to_string(),
            task_id: None,
            last_state_line: 0,
            task_title: "idle".to_string(),
            tool_line: "waiting for merge".to_string(),
            breadcrumb: "idle".to_string(),
            last_heartbeat_secs: 0,
            session_age_secs: 0,
            lease_held: false,
            session_missing: false,
            command_details: Vec::new(),
        };
        set_worker_idle(&mut row, "waiting for merge");
        row
    });
    let mut last_worker_state_line = 0usize;
    let mut event_sequence = 0usize;
    let mut last_activity_pulse = vec![Instant::now(); workers.len()];
    let mut completed = 0usize;
    let mut merged = 0usize;
    let mut failed = 0usize;
    let run_started = Instant::now();
    let run_started_at_ms = now_unix_millis();

    refresh_worker_heartbeats(&mut workers, &last_activity_pulse);
    render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;

    while completed < target {
        if handle_hotkeys(&mut HotkeyState {
            runtime,
            scope,
            cfg,
            store,
            workers: &mut workers,
            operator_hotkeys,
            terminal,
            report_visible: &mut report_visible,
            report_content: &mut report_content,
        })? {
            return Ok(completed);
        }
        if report_visible {
            continue;
        }
        let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);
        let worktree_client = WorktreeClient::new(runtime.process_runner.as_ref(), repo_root);
        let epoch = run_pool_epoch(
            runtime,
            scope,
            cfg,
            store,
            terminal,
            &mut workers,
            operator_hotkeys,
            &mut report_visible,
            &mut report_content,
            merge_row_idx,
            &mut last_worker_state_line,
            &mut event_sequence,
            &mut last_activity_pulse,
            parallelism,
            target,
            task_override,
            run_started_at_ms,
            repo_root,
            &worktree_client,
            &mut completed,
            &mut merged,
            &mut failed,
            hb,
            lt,
        )?;
        if epoch.exhausted_backlog {
            break;
        }

        let summary = ShutdownSummary {
            completed,
            target,
            merged,
            failed,
            total_runtime_secs: run_started.elapsed().as_secs(),
        };

        if let Some((worker_id, task_id, reason)) = epoch.shutdown_error {
            let error_detail = worker_failure_prompt(&worker_id, &task_id, &reason);
            let shutdown_message = format!("{}\n\n{}", error_detail, summary.format_message());
            if terminal.stdin_is_tty() {
                terminal.draw_shutdown_screen("Error", &shutdown_message)?;
            } else {
                terminal.write_line(&format!(
                    "Error: worker {worker_id} task {task_id}: {reason}\n{}",
                    summary.format_message()
                ))?;
            }
            wait_for_quit(terminal, Some(&shutdown_message))?;
            return Ok(completed);
        }
        if epoch.quit_requested {
            append_run_log(
                "info",
                "worker_pool.quit_requested",
                json!({
                    "worker_id": WORKER_POOL_ID,
                    "completed": completed,
                    "target": target,
                    "merged": merged,
                    "failed": failed
                }),
            );
            let shutdown_title = "Session Interrupted";
            let shutdown_message = summary.format_message();
            if terminal.stdin_is_tty() {
                terminal.draw_shutdown_screen(shutdown_title, &shutdown_message)?;
            } else {
                terminal.write_line(&format!("{shutdown_title}: {shutdown_message}"))?;
            }
            wait_for_quit(terminal, None)?;
            return Ok(completed);
        }
    }
    let summary = ShutdownSummary {
        completed,
        target,
        merged,
        failed,
        total_runtime_secs: run_started.elapsed().as_secs(),
    };
    append_run_log(
        "info",
        "worker_pool.completed",
        json!({
            "worker_id": WORKER_POOL_ID,
            "completed": completed,
            "target": target,
            "merged": merged,
            "failed": failed
        }),
    );
    let shutdown_title = if completed >= target {
        "All Tasks Complete"
    } else if completed == 0 {
        "Empty Backlog"
    } else {
        "No More Work"
    };
    let shutdown_message = summary.format_message();
    if terminal.stdin_is_tty() {
        terminal.draw_shutdown_screen(shutdown_title, &shutdown_message)?;
    } else {
        terminal.write_line(&format!("{shutdown_title}: {shutdown_message}"))?;
    }
    wait_for_quit(terminal, None)?;
    Ok(completed)
}

#[cfg(test)]
// hotkey:q hotkey:j hotkey:k hotkey:v hotkey:g hotkey:b
mod tests;
