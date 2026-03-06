use crate::backlog_store::BacklogStore;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::hotkeys::operator_hotkeys_enabled;
use crate::logging::append_run_log;
use crate::runtime::Terminal;
use crate::runtime::{clear_interrupt, request_interrupt, ProductionRuntime};
use crate::tui::{reset_workers_scroll, WorkerRow};
use crate::types::RuntimeScope;
use crate::worker::{
    clear_state_sink, execute_merge_phase, execute_task, install_state_sink, MergeRequest,
    WorkerOutcome, WorkerStreamEvent,
};
use crate::worktree::WorktreeClient;
use serde_json::json;
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod dashboard;
mod event_handling;
mod hotkeys;
mod result_handling;
mod scheduling;
mod util;

use dashboard::{dashboard_snapshot, render, worker_failure_prompt};
use event_handling::drain_pool_events;
use hotkeys::{handle_hotkeys, wait_for_quit, HotkeyState};
use result_handling::{
    handle_doing_complete_transition, handle_doing_non_complete_transition, handle_merge_summary,
};
use scheduling::{
    append_worker_command, claim_tasks_for_available_workers, execution_task_packet,
    mark_merge_worker_busy, maybe_start_merge, refresh_worker_heartbeats, set_worker_idle,
};
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
        let mut claimed = Vec::new();
        let mut active_merging = 0usize;
        let (tx, rx): (
            mpsc::Sender<PoolResultMessage>,
            mpsc::Receiver<PoolResultMessage>,
        ) = mpsc::channel();
        let (event_tx, event_rx): (
            mpsc::Sender<PoolStreamEvent>,
            mpsc::Receiver<PoolStreamEvent>,
        ) = mpsc::channel();
        // Merge queue: doing workers send MergeRequests here.
        let (merge_tx, merge_rx): (mpsc::Sender<MergeRequest>, mpsc::Receiver<MergeRequest>) =
            mpsc::channel();
        let runtime_scope = scope.clone();
        let mut merge_tx = Some(merge_tx);
        let mut claimed_any = false;
        if let Some((pr_number, task_id, task_summary)) = maybe_start_merge(
            &mut active_merging,
            &mut merge_tx,
            store,
            repo_root,
            &worktree_client,
        )? {
            claimed_any = true;
            mark_merge_worker_busy(
                &mut workers,
                &mut last_activity_pulse,
                merge_row_idx,
                pr_number,
                &task_id,
                last_worker_state_line,
                &task_summary,
            );
        }
        claimed_any = claim_tasks_for_available_workers(
            &mut workers,
            &mut claimed,
            completed,
            active_merging,
            last_worker_state_line,
            &mut last_activity_pulse,
            parallelism,
            target,
            run_started_at_ms,
            store,
            cfg,
            terminal,
            hb,
            lt,
        )? || claimed_any;

        if !claimed_any && active_merging == 0 {
            let mut claimed_on_idle = false;
            for _ in 0..IDLE_WORKER_POLL_ATTEMPTS {
                std::thread::sleep(Duration::from_millis(IDLE_WORKER_POLL_DELAY_MS));
                claimed_any = claim_tasks_for_available_workers(
                    &mut workers,
                    &mut claimed,
                    completed,
                    active_merging,
                    last_worker_state_line,
                    &mut last_activity_pulse,
                    parallelism,
                    target,
                    run_started_at_ms,
                    store,
                    cfg,
                    terminal,
                    hb,
                    lt,
                )?;
                if claimed_any {
                    claimed_on_idle = true;
                    break;
                }
            }
            if !claimed_on_idle {
                break;
            }
        }

        let mut active_doing = claimed.len();
        let mut shutdown_error: Option<(String, String, String)> = None;
        let mut quit_requested = false;
        let mut quit_requested_at: Option<Instant> = None;
        let mut last_shutdown_log = Instant::now();
        let mut last_dashboard_refresh = Instant::now();
        let mut last_render_completed: Option<Instant> = None;
        let mut last_render_heartbeat = Instant::now() - Duration::from_secs(60);

        std::thread::scope(|scope_guard| -> Result<(), GardenerError> {
            // Spawn doing workers.
            for (idx, task) in claimed {
                let tx = tx.clone();
                let event_tx = event_tx.clone();
                let worker_id = workers[idx].worker_id.clone();
                let task_id = task.task_id.clone();
                let task_summary = execution_task_packet(&task, task_override);
                let attempt_count = task.attempt_count;
                let task_created_at = task.created_at;
                let task_last_updated = task.last_updated;
                let cfg = cfg.clone();
                let process_runner = runtime.process_runner.clone();
                let worker_scope = runtime_scope.clone();
                scope_guard.spawn(move || {
                    let state_ui_tx = event_tx.clone();
                    let on_event = move |event: WorkerStreamEvent| {
                        if let WorkerStreamEvent::ToolCommand { task_id, command } = event {
                            let _ = event_tx.send(PoolStreamEvent::ToolCommand {
                                slot_idx: idx,
                                task_id,
                                command,
                            });
                        }
                    };
                    install_state_sink(Box::new(move |state, task_id, details| {
                        let _ = state_ui_tx.send(PoolStreamEvent::StateChanged {
                            slot_idx: idx,
                            task_id: task_id.to_string(),
                            state: state.to_string(),
                            details: details.to_string(),
                        });
                    }));
                    let result = execute_task(
                        &cfg,
                        process_runner.as_ref(),
                        &worker_scope,
                        idx,
                        &worker_id,
                        &task_id,
                        &task_summary,
                        attempt_count,
                        run_started_at_ms,
                        task_created_at,
                        task_last_updated,
                        Some(&on_event),
                    );
                    clear_state_sink();
                    let _ = tx.send(PoolResultMessage::DoingResult {
                        slot_idx: idx,
                        task_id,
                        result,
                    });
                });
            }

            // Spawn the merge worker thread.
            {
                let merge_result_tx = tx.clone();
                let merge_cfg = cfg.clone();
                let merge_runner = runtime.process_runner.clone();
                let merge_scope = runtime_scope.clone();
                let merge_file_system = runtime.file_system.clone();
                let merge_clock = runtime.clock.clone();
                let event_tx = event_tx.clone();
                scope_guard.spawn(move || {
                    let event_tx = event_tx.clone();
                    while let Ok(req) = merge_rx.recv() {
                        let state_ui_tx = event_tx.clone();
                        let event_tx = event_tx.clone();
                        let task_id = req.task_id.clone();
                        let on_event = move |event: WorkerStreamEvent| {
                            if let WorkerStreamEvent::ToolCommand { task_id, command } = event {
                                let _ = event_tx.send(PoolStreamEvent::ToolCommand {
                                    slot_idx: merge_row_idx,
                                    task_id,
                                    command,
                                });
                            }
                        };
                        install_state_sink(Box::new(move |state, task_id, details| {
                            let _ = state_ui_tx.send(PoolStreamEvent::StateChanged {
                                slot_idx: merge_row_idx,
                                task_id: task_id.to_string(),
                                state: state.to_string(),
                                details: details.to_string(),
                            });
                        }));
                        let result = execute_merge_phase(
                            &req,
                            &merge_cfg,
                            merge_runner.as_ref(),
                            merge_file_system.as_ref(),
                            merge_clock.as_ref(),
                            &merge_scope,
                            Some(&on_event),
                        );
                        clear_state_sink();
                        let _ = merge_result_tx
                            .send(PoolResultMessage::MergeResult { task_id, result });
                    }
                });
            }
            // Keep merge_tx alive so the pool can send MergeRequests when
            // doing workers return HandoffToMerge. Drop it when all doing
            // workers finish so the merge worker sees channel-close and exits.
            let mut merge_tx = merge_tx;

            while active_doing > 0 || active_merging > 0 {
                let mut updated = drain_pool_events(
                    &mut workers,
                    &mut last_activity_pulse,
                    &mut event_sequence,
                    &event_rx,
                );
                if handle_hotkeys(&mut HotkeyState {
                    runtime,
                    scope: &runtime_scope,
                    cfg,
                    store,
                    workers: &mut workers,
                    operator_hotkeys,
                    terminal,
                    report_visible: &mut report_visible,
                    report_content: &mut report_content,
                })? {
                    request_interrupt();
                    if !quit_requested {
                        quit_requested_at = Some(Instant::now());
                        last_shutdown_log = Instant::now();
                        append_run_log(
                            "warn",
                            "worker_pool.shutdown.requested",
                            json!({
                                "worker_id": WORKER_POOL_ID,
                                "active_doing": active_doing,
                                "active_merging": active_merging,
                            }),
                        );
                    }
                    quit_requested = true;
                }

                match rx.recv_timeout(Duration::from_millis(25)) {
                    Ok(PoolResultMessage::DoingResult {
                        slot_idx: idx,
                        task_id,
                        result,
                    }) => {
                        active_doing = active_doing.saturating_sub(1);
                        let worker_id = workers[idx].worker_id.clone();
                        if shutdown_error.is_some() {
                            request_interrupt();
                        } else {
                            match result {
                                Err(GardenerError::Process(message))
                                    if message.contains("user interrupt requested") =>
                                {
                                    quit_requested = true;
                                }
                                Err(err) => {
                                    let msg = err.to_string();
                                    append_run_log(
                                        "error",
                                        "worker.task.process_error",
                                        json!({
                                            "worker_id": worker_id,
                                            "task_id": task_id,
                                            "error": msg
                                        }),
                                    );
                                    shutdown_error =
                                        Some((worker_id.clone(), task_id.clone(), msg));
                                    request_interrupt();
                                }
                                Ok(WorkerOutcome::HandoffToMerge(req)) => {
                                    // Transition backlog: in_progress → merge_pending
                                    let moved_to_merge_pending =
                                        store.mark_merge_pending(&task_id, &worker_id)?;
                                    if !moved_to_merge_pending {
                                        append_run_log(
                                            "error",
                                            "worker.task.handoff_transition_rejected",
                                            json!({
                                                "worker_id": worker_id,
                                                "task_id": task_id,
                                                "transition": "mark_merge_pending",
                                            }),
                                        );
                                        let _ = store.mark_unresolved(&task_id, &worker_id);
                                        failed = failed.saturating_add(1);
                                        workers[idx].state = "failed".to_string();
                                        workers[idx].task_id = None;
                                        workers[idx].last_state_line = last_worker_state_line;
                                        let fail_msg = format!(
                                            "handoff failed (transition rejected) {}",
                                            task_id
                                        );
                                        workers[idx].tool_line = fail_msg.clone();
                                        append_worker_command(&mut workers[idx], &fail_msg);
                                        workers[idx].breadcrumb = "failed".to_string();
                                        workers[idx].lease_held = false;
                                        continue;
                                    }
                                    let linked_pr = store.set_related_pr(
                                        &task_id,
                                        req.pr_number as i64,
                                        &req.branch,
                                    )?;
                                    if !linked_pr {
                                        append_run_log(
                                            "error",
                                            "worker.task.handoff_related_pr_rejected",
                                            json!({
                                                "worker_id": worker_id,
                                                "task_id": task_id,
                                                "pr_number": req.pr_number,
                                                "branch": &req.branch,
                                            }),
                                        );
                                    }
                                    // Process doing worker logs for TUI
                                    for event in &req.logs {
                                        workers[idx].state = event.state.as_str().to_string();
                                        let prompt = format!("prompt {}", event.prompt_version);
                                        workers[idx].tool_line = prompt.clone();
                                        append_worker_command(&mut workers[idx], &prompt);
                                        workers[idx].breadcrumb =
                                            format!("state>{}", event.state.as_str());
                                        last_activity_pulse[idx] = Instant::now();
                                    }
                                    // Update doing worker TUI to show handoff
                                    workers[idx].state = "merging".to_string();
                                    let handoff_msg =
                                        format!("handed off PR #{} to merge worker", req.pr_number);
                                    workers[idx].tool_line = handoff_msg.clone();
                                    append_worker_command(&mut workers[idx], &handoff_msg);
                                    workers[idx].breadcrumb = "handoff>merge".to_string();
                                    workers[idx].lease_held = false;
                                    workers[idx].last_state_line = last_worker_state_line;
                                    if let Some((pr_number, task_id, task_summary)) =
                                        maybe_start_merge(
                                            &mut active_merging,
                                            &mut merge_tx,
                                            store,
                                            repo_root,
                                            &worktree_client,
                                        )?
                                    {
                                        mark_merge_worker_busy(
                                            &mut workers,
                                            &mut last_activity_pulse,
                                            merge_row_idx,
                                            pr_number,
                                            &task_id,
                                            last_worker_state_line,
                                            &task_summary,
                                        );
                                    }
                                    append_run_log(
                                        "info",
                                        "worker.task.handoff_to_merge",
                                        json!({
                                            "worker_id": worker_id,
                                            "task_id": task_id,
                                            "active_merging": active_merging
                                        }),
                                    );
                                }
                                Ok(WorkerOutcome::Completed(summary)) => {
                                    for event in &summary.logs {
                                        workers[idx].state = event.state.as_str().to_string();
                                        let prompt = format!("prompt {}", event.prompt_version);
                                        workers[idx].tool_line = prompt.clone();
                                        append_worker_command(&mut workers[idx], &prompt);
                                        workers[idx].breadcrumb =
                                            format!("state>{}", event.state.as_str());
                                        let now = Instant::now();
                                        last_activity_pulse[idx] = now;
                                        workers[idx].last_heartbeat_secs = 0;
                                        workers[idx].session_age_secs = 0;
                                        append_run_log(
                                            "debug",
                                            "worker.turn.state",
                                            json!({
                                                "worker_id": worker_id,
                                                "state": event.state.as_str(),
                                                "prompt_version": event.prompt_version,
                                                "context_manifest_hash": event.context_manifest_hash
                                            }),
                                        );
                                        refresh_worker_heartbeats(
                                            &mut workers,
                                            &last_activity_pulse,
                                        );
                                        render(
                                            terminal,
                                            &workers,
                                            &dashboard_snapshot(store)?,
                                            hb,
                                            lt,
                                        )?;
                                    }
                                    if summary.final_state == crate::types::WorkerState::Complete {
                                        let _ = handle_doing_complete_transition(
                                            store,
                                            &mut workers,
                                            idx,
                                            last_worker_state_line,
                                            &worker_id,
                                            &task_id,
                                            &mut completed,
                                            &mut failed,
                                        )?;
                                    } else {
                                        let _ = handle_doing_non_complete_transition(
                                            store,
                                            &mut workers,
                                            idx,
                                            last_worker_state_line,
                                            &worker_id,
                                            &task_id,
                                            &summary,
                                            &mut failed,
                                        )?;
                                    }
                                }
                            }
                        }

                        // Re-claim for doing worker slot
                        if shutdown_error.is_none()
                            && !quit_requested
                            && completed
                                .saturating_add(active_doing)
                                .saturating_add(active_merging)
                                < target
                        {
                            let worker_id = workers[idx].worker_id.clone();
                            let claimed_task = store.claim_next(
                                &worker_id,
                                cfg.scheduler.lease_timeout_seconds as i64,
                            )?;
                            if let Some(task) = claimed_task {
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
                                let moved_to_in_progress =
                                    store.mark_in_progress(&task.task_id, &worker_id)?;
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
                                let now = Instant::now();
                                last_activity_pulse[idx] = now;
                                workers[idx].last_heartbeat_secs = 0;
                                workers[idx].session_age_secs = 0;
                                refresh_worker_heartbeats(&mut workers, &last_activity_pulse);
                                render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;
                                last_render_completed = Some(Instant::now());

                                let tx = tx.clone();
                                let task_id = task.task_id.clone();
                                let task_summary = execution_task_packet(&task, task_override);
                                let attempt_count = task.attempt_count;
                                let task_created_at = task.created_at;
                                let task_last_updated = task.last_updated;
                                let cfg = cfg.clone();
                                let process_runner = runtime.process_runner.clone();
                                let worker_scope = runtime_scope.clone();
                                let event_tx = event_tx.clone();
                                scope_guard.spawn(move || {
                                    let state_ui_tx = event_tx.clone();
                                    let on_event = move |event: WorkerStreamEvent| {
                                        if let WorkerStreamEvent::ToolCommand { task_id, command } =
                                            event
                                        {
                                            let _ = event_tx.send(PoolStreamEvent::ToolCommand {
                                                slot_idx: idx,
                                                task_id,
                                                command,
                                            });
                                        }
                                    };
                                    install_state_sink(Box::new(move |state, task_id, details| {
                                        let _ = state_ui_tx.send(PoolStreamEvent::StateChanged {
                                            slot_idx: idx,
                                            task_id: task_id.to_string(),
                                            state: state.to_string(),
                                            details: details.to_string(),
                                        });
                                    }));
                                    let result = execute_task(
                                        &cfg,
                                        process_runner.as_ref(),
                                        &worker_scope,
                                        idx,
                                        &worker_id,
                                        &task_id,
                                        &task_summary,
                                        attempt_count,
                                        run_started_at_ms,
                                        task_created_at,
                                        task_last_updated,
                                        Some(&on_event),
                                    );
                                    clear_state_sink();
                                    let _ = tx.send(PoolResultMessage::DoingResult {
                                        slot_idx: idx,
                                        task_id,
                                        result,
                                    });
                                });
                                active_doing = active_doing.saturating_add(1);
                            } else {
                                set_worker_idle(&mut workers[idx], "waiting for claim");
                            }
                        }
                        // Signal merge worker to exit when all work is done
                        if active_doing == 0 && active_merging == 0 {
                            merge_tx.take();
                        }
                        refresh_worker_heartbeats(&mut workers, &last_activity_pulse);
                        render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;
                        last_render_completed = Some(Instant::now());
                    }
                    Ok(PoolResultMessage::MergeResult { task_id, result }) => {
                        active_merging = active_merging.saturating_sub(1);
                        if shutdown_error.is_some() {
                            request_interrupt();
                        } else {
                            match result {
                                Err(GardenerError::Process(message))
                                    if message.contains("user interrupt requested") =>
                                {
                                    // Graceful shutdown: restore to merge_pending so the next
                                    // run picks this task up again. claim_merge_pending sets no
                                    // lease_expires_at, so stale-lease recovery won't help here.
                                    append_run_log(
                                        "warn",
                                        "merge_worker.task.interrupted",
                                        json!({
                                            "worker_id": MERGE_WORKER_ID,
                                            "task_id": task_id
                                        }),
                                    );
                                    let _ = store.mark_merge_pending(&task_id, MERGE_WORKER_ID);
                                    workers[merge_row_idx].state = "interrupted".to_string();
                                    let interrupted_msg =
                                        format!("interrupted (merge_pending) {}", task_id);
                                    workers[merge_row_idx].tool_line = interrupted_msg.clone();
                                    append_worker_command(
                                        &mut workers[merge_row_idx],
                                        &interrupted_msg,
                                    );
                                    workers[merge_row_idx].last_state_line = last_worker_state_line;
                                }
                                Err(err) => {
                                    let msg = err.to_string();
                                    failed = failed.saturating_add(1);
                                    append_run_log(
                                        "error",
                                        "merge_worker.task.process_error",
                                        json!({
                                            "worker_id": MERGE_WORKER_ID,
                                            "task_id": task_id,
                                            "error": msg
                                        }),
                                    );
                                    let _ = store.mark_unresolved(&task_id, MERGE_WORKER_ID);
                                    workers[merge_row_idx].state = "failed".to_string();
                                    let fail_msg = format!(
                                        "merge failed: {}",
                                        msg.chars().take(100).collect::<String>()
                                    );
                                    workers[merge_row_idx].tool_line = fail_msg.clone();
                                    append_worker_command(&mut workers[merge_row_idx], &fail_msg);
                                    workers[merge_row_idx].last_state_line = last_worker_state_line;
                                }
                                Ok(summary) => {
                                    let handling = handle_merge_summary(
                                        store,
                                        &mut workers,
                                        merge_row_idx,
                                        last_worker_state_line,
                                        &task_id,
                                        &summary,
                                        &mut completed,
                                        &mut merged,
                                        &mut failed,
                                    )?;
                                    if handling == MergeSummaryHandling::EarlyContinue {
                                        continue;
                                    }
                                }
                            }
                        }
                        if let Some((pr_number, task_id, task_summary)) = maybe_start_merge(
                            &mut active_merging,
                            &mut merge_tx,
                            store,
                            repo_root,
                            &worktree_client,
                        )? {
                            mark_merge_worker_busy(
                                &mut workers,
                                &mut last_activity_pulse,
                                merge_row_idx,
                                pr_number,
                                &task_id,
                                last_worker_state_line,
                                &task_summary,
                            );
                        }
                        // Reset merge row to idle if no more merges pending
                        if active_merging == 0 {
                            set_worker_idle(&mut workers[merge_row_idx], "waiting for merge");
                            workers[merge_row_idx].task_id = None;
                        }
                        refresh_worker_heartbeats(&mut workers, &last_activity_pulse);
                        render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;
                        last_render_completed = Some(Instant::now());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if drain_pool_events(
                            &mut workers,
                            &mut last_activity_pulse,
                            &mut event_sequence,
                            &event_rx,
                        ) {
                            updated = true;
                        }
                        if last_worker_state_line < event_sequence {
                            last_worker_state_line = event_sequence;
                        }
                        if updated || last_dashboard_refresh.elapsed() >= Duration::from_secs(1) {
                            refresh_worker_heartbeats(&mut workers, &last_activity_pulse);
                            let snapshot = dashboard_snapshot(store)?;
                            if last_render_heartbeat.elapsed() >= Duration::from_secs(60) {
                                last_render_heartbeat = Instant::now();
                                append_run_log(
                                    "debug",
                                    "dashboard.snapshot",
                                    json!({
                                        "worker_id": WORKER_POOL_ID,
                                        "workers": workers.len(),
                                        "active": snapshot.stats.active,
                                        "ready": snapshot.stats.ready,
                                        "failed": snapshot.stats.failed,
                                        "unresolved": snapshot.stats.unresolved,
                                        "tty": terminal.stdin_is_tty(),
                                        "last_render_ago_ms": last_render_completed
                                            .map(|t| t.elapsed().as_millis() as u64)
                                            .unwrap_or(0),
                                    }),
                                );
                            }
                            render(terminal, &workers, &snapshot, hb, lt)?;
                            last_render_completed = Some(Instant::now());
                            last_dashboard_refresh = Instant::now();
                        }
                        if quit_requested && last_shutdown_log.elapsed() >= Duration::from_secs(5) {
                            last_shutdown_log = Instant::now();
                            append_run_log(
                                "warn",
                                "worker_pool.shutdown.waiting",
                                json!({
                                    "worker_id": WORKER_POOL_ID,
                                    "active_doing": active_doing,
                                    "active_merging": active_merging,
                                    "elapsed_ms": quit_requested_at
                                        .map(|t| t.elapsed().as_millis() as u64)
                                        .unwrap_or(0),
                                }),
                            );
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        active_doing = 0;
                        active_merging = 0;
                    }
                }
            }
            Ok(())
        })?;

        let summary = ShutdownSummary {
            completed,
            target,
            merged,
            failed,
            total_runtime_secs: run_started.elapsed().as_secs(),
        };

        if let Some((worker_id, task_id, reason)) = shutdown_error {
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
        if quit_requested {
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
mod tests {
    use super::event_handling::{apply_pool_stream_event, is_non_regressive_state_transition};
    use super::hotkeys::wait_for_quit;
    use super::result_handling::{
        handle_doing_complete_transition, handle_doing_non_complete_transition,
        handle_merge_summary,
    };
    use super::scheduling::execution_task_packet;
    use super::{
        available_doing_slots, run_worker_pool_fsm, DoingSummaryHandling, MergeSummaryHandling,
        PoolStreamEvent,
    };
    use crate::backlog_store::{BacklogStore, BacklogTask, NewTask, TaskStatus};
    use crate::config::AppConfig;
    use crate::hotkeys::{
        action_for_key, action_for_key_with_mode, HotkeyAction, DASHBOARD_BINDINGS, REPORT_BINDINGS,
    };
    use crate::logging::{clear_run_logger, init_run_logger};
    use crate::priority::Priority;
    use crate::runtime::{
        FakeClock, FakeProcessRunner, FakeTerminal, ProductionFileSystem, ProductionRuntime,
        INTERRUPT_SENTINEL_KEY,
    };
    use crate::task_identity::TaskKind;
    use crate::tui::WorkerRow;
    use crate::types::{RuntimeScope, WorkerState};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn seed_task(store: &BacklogStore, title: &str) {
        let _ = store
            .upsert_task(NewTask {
                kind: TaskKind::Maintenance,
                title: title.to_string(),
                details: "details".to_string(),
                scope_key: "scope".to_string(),
                rationale: "seeded for unit/integration test visibility".to_string(),
                priority: Priority::P1,
                source: "test".to_string(),
                related_pr: None,
                related_branch: None,
            })
            .expect("seed task");
    }

    fn seed_merge_pending_without_pr(
        store: &BacklogStore,
        title: &str,
        lease_owner: &str,
    ) -> String {
        let row = store
            .upsert_task(NewTask {
                kind: TaskKind::Maintenance,
                title: title.to_string(),
                details: "details".to_string(),
                scope_key: "scope".to_string(),
                rationale: "seeded merge_pending regression task".to_string(),
                priority: Priority::P1,
                source: "test".to_string(),
                related_pr: None,
                related_branch: None,
            })
            .expect("seed merge-pending task");
        let claimed = store
            .claim_next(lease_owner, 300)
            .expect("claim seeded task")
            .expect("task claimed");
        assert_eq!(claimed.task_id, row.task_id);
        assert!(store
            .mark_in_progress(&row.task_id, lease_owner)
            .expect("mark in progress"));
        assert!(store
            .mark_merge_pending(&row.task_id, lease_owner)
            .expect("mark merge pending"));
        row.task_id
    }

    fn test_scope(dir: &TempDir) -> RuntimeScope {
        RuntimeScope {
            process_cwd: dir.path().to_path_buf(),
            repo_root: Some(dir.path().to_path_buf()),
            working_dir: dir.path().to_path_buf(),
        }
    }

    fn write_file(path: &PathBuf, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, contents).expect("write file");
    }

    #[test]
    fn execution_task_packet_includes_details_when_present() {
        let task = BacklogTask {
            task_id: "manual:test:task-1".to_string(),
            kind: TaskKind::Maintenance,
            title: "Implement direct event streaming".to_string(),
            details: "Plan: thoughts/shared/plans/2026-03-02-direct-event-streaming-worker-ui.md"
                .to_string(),
            rationale: String::new(),
            scope_key: "runtime".to_string(),
            priority: Priority::P0,
            status: TaskStatus::Ready,
            last_updated: 0,
            lease_owner: None,
            lease_expires_at: None,
            source: "test".to_string(),
            related_pr: None,
            related_branch: None,
            attempt_count: 1,
            created_at: 0,
        };

        let packet = execution_task_packet(&task, None);
        assert!(packet.starts_with(&task.title));
        assert!(packet.contains(&task.details));
        assert!(packet.contains("\n\n"));

        let override_packet = execution_task_packet(&task, Some("override summary"));
        assert_eq!(override_packet, "override summary");
    }

    #[test]
    fn report_hotkey_actions_cover_report_bindings() {
        for binding in REPORT_BINDINGS {
            let action = action_for_key_with_mode(binding.key, false);
            assert!(action.is_some());
        }
    }

    #[test]
    fn hotkey_actions_match_default_and_operator_contracts() {
        assert_eq!(
            action_for_key_with_mode('q', false),
            Some(HotkeyAction::Quit)
        ); // hotkey:q
        assert_eq!(
            action_for_key_with_mode('j', false),
            Some(HotkeyAction::ScrollDown)
        ); // hotkey:j
        assert_eq!(
            action_for_key_with_mode('k', false),
            Some(HotkeyAction::ScrollUp)
        ); // hotkey:k
        assert_eq!(action_for_key_with_mode('c', false), None); // hotkey:c removed
        assert_eq!(
            action_for_key_with_mode('v', false),
            Some(HotkeyAction::ViewReport)
        ); // hotkey:v
        assert_eq!(
            action_for_key_with_mode('g', false),
            Some(HotkeyAction::RegenerateReport)
        ); // hotkey:g
        assert_eq!(
            action_for_key_with_mode('b', false),
            Some(HotkeyAction::Back)
        ); // hotkey:b
        assert_eq!(action_for_key_with_mode('r', false), None);
        assert_eq!(action_for_key_with_mode('l', false), None);
        assert_eq!(action_for_key_with_mode('p', false), None);
        assert_eq!(action_for_key_with_mode('c', true), None); // hotkey:c removed

        assert_eq!(
            action_for_key_with_mode('r', true),
            Some(HotkeyAction::Retry)
        ); // hotkey:r
        assert_eq!(
            action_for_key_with_mode('l', true),
            Some(HotkeyAction::ReleaseLease)
        ); // hotkey:l
        assert_eq!(
            action_for_key_with_mode('p', true),
            Some(HotkeyAction::ParkEscalate)
        ); // hotkey:p
        assert_eq!(action_for_key_with_mode('x', true), None);
    }

    #[test]
    fn all_advertised_hotkeys_have_actions() {
        for binding in DASHBOARD_BINDINGS {
            assert!(action_for_key(binding.key).is_some());
        }
        for binding in REPORT_BINDINGS {
            assert!(action_for_key(binding.key).is_some());
        }
    }

    #[test]
    fn run_worker_pool_fsm_switches_between_dashboard_and_report_frames() {
        let dir = TempDir::new().expect("tempdir");
        let scope = test_scope(&dir);

        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        seed_task(&store, "snapshot task");

        let quality_path = dir.path().join(".gardener/quality.md");
        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.orchestrator.parallelism = 1;
        cfg.quality_report.path = quality_path.display().to_string();

        write_file(&quality_path, "overall: A+");

        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys(['v', 'b']);

        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(terminal.clone()),
        };

        let completed = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 1, None)
            .expect("run fsm");
        assert_eq!(completed, 1);

        let frames = terminal.drawn_frames();
        let dashboard_frames = frames
            .iter()
            .filter(|frame| frame.contains("GARDENER live queue"))
            .count();
        let report_frames = frames
            .iter()
            .filter(|frame| frame.contains("Quality report view"))
            .count();
        assert!(
            dashboard_frames >= 2,
            "expected at least 2 dashboard renders (initial and after back): {dashboard_frames}"
        );
        assert!(
            report_frames >= 1,
            "expected at least one report render: {report_frames}"
        );
        assert!(!terminal.report_draws().is_empty());
    }

    #[test]
    fn run_worker_pool_fsm_handles_v_and_b_with_report_draws() {
        let dir = TempDir::new().expect("tempdir");
        let scope = test_scope(&dir);

        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        seed_task(&store, "hotkey task");

        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.orchestrator.parallelism = 1;
        cfg.triage.output_path = dir
            .path()
            .join(".gardener/repo-intelligence.toml")
            .display()
            .to_string();
        cfg.quality_report.path = dir
            .path()
            .join(".gardener/quality.md")
            .display()
            .to_string();

        write_file(
            &dir.path().join(".gardener/repo-intelligence.toml"),
            include_str!("../../tests/fixtures/triage/expected-profiles/phase03-profile.toml"),
        );
        write_file(&dir.path().join(".gardener/quality.md"), "existing report");

        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys(['v', 'b']);

        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(terminal.clone()),
        };

        let completed = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 1, None)
            .expect("run fsm");
        assert_eq!(completed, 1);
        assert!(!terminal.report_draws().is_empty());
    }

    #[test]
    fn run_worker_pool_fsm_handles_g_and_regenerates_report() {
        let dir = TempDir::new().expect("tempdir");
        let scope = test_scope(&dir);

        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        seed_task(&store, "regenerate report task");

        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.orchestrator.parallelism = 1;
        cfg.triage.output_path = dir
            .path()
            .join(".gardener/repo-intelligence.toml")
            .display()
            .to_string();
        cfg.quality_report.path = dir
            .path()
            .join(".gardener/quality.md")
            .display()
            .to_string();

        write_file(
            &dir.path().join(".gardener/repo-intelligence.toml"),
            include_str!("../../tests/fixtures/triage/expected-profiles/phase03-profile.toml"),
        );
        write_file(&dir.path().join(".gardener/quality.md"), "OLD_MARKER");

        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys(['g', 'b']);

        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(terminal.clone()),
        };

        let completed = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 1, None)
            .expect("run fsm");
        assert_eq!(completed, 1);
        let report = std::fs::read_to_string(dir.path().join(".gardener/quality.md"))
            .expect("read regenerated report");
        assert!(!report.contains("OLD_MARKER"));
        assert!(!terminal.report_draws().is_empty());
    }

    #[test]
    fn run_worker_pool_fsm_claims_tasks_inserted_while_idle() {
        let dir = TempDir::new().expect("tempdir");
        let scope = test_scope(&dir);

        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        seed_task(&store, "initial task");

        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.orchestrator.parallelism = 1;
        cfg.quality_report.path = dir
            .path()
            .join(".gardener/quality.md")
            .display()
            .to_string();

        let inserter_path = db_path;
        let inserter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            let inserter_store = BacklogStore::open(&inserter_path).expect("open inserter store");
            let _ = inserter_store
                .upsert_task(NewTask {
                    kind: TaskKind::Maintenance,
                    title: "late task".to_string(),
                    details: "inserted after start".to_string(),
                    scope_key: "scope".to_string(),
                    rationale: "inserted by runtime test thread".to_string(),
                    priority: Priority::P1,
                    source: "test".to_string(),
                    related_pr: None,
                    related_branch: None,
                })
                .expect("insert late task");
        });

        let terminal = FakeTerminal::new(true);
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(terminal.clone()),
        };

        let completed = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 2, None)
            .expect("run fsm");
        inserter.join().expect("inserter thread completed");

        assert_eq!(completed, 2);

        let tasks = store.list_tasks().expect("list tasks");
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.title == "late task")
                .count(),
            1
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.status == crate::backlog_store::TaskStatus::Complete)
                .count(),
            2
        );
    }

    #[test]
    fn run_worker_pool_fsm_skips_invalid_merge_pending_rows_in_same_cycle() {
        let dir = TempDir::new().expect("tempdir");
        let scope = test_scope(&dir);
        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        let first = seed_merge_pending_without_pr(&store, "invalid merge task 1", "seed-1");
        let second = seed_merge_pending_without_pr(&store, "invalid merge task 2", "seed-2");

        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.orchestrator.parallelism = 1;
        cfg.quality_report.path = dir
            .path()
            .join(".gardener/quality.md")
            .display()
            .to_string();

        let terminal = FakeTerminal::new(false);
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(terminal.clone()),
        };

        let _ = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 1, None)
            .expect("run fsm");

        let first_task = store
            .get_task(&first)
            .expect("fetch first task")
            .expect("first task exists");
        let second_task = store
            .get_task(&second)
            .expect("fetch second task")
            .expect("second task exists");
        assert_ne!(first_task.status, TaskStatus::MergePending);
        assert_ne!(second_task.status, TaskStatus::MergePending);
        let remaining_merge_pending = store
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .filter(|task| task.status == TaskStatus::MergePending)
            .count();
        assert_eq!(remaining_merge_pending, 0);
    }

    #[test]
    fn run_worker_pool_fsm_quits_on_q() {
        let dir = TempDir::new().expect("tempdir");
        let scope = test_scope(&dir);
        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        seed_task(&store, "quit task");

        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.orchestrator.parallelism = 1;
        cfg.quality_report.path = dir
            .path()
            .join(".gardener/quality.md")
            .display()
            .to_string();

        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys(['q']);
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(terminal.clone()),
        };

        let _ = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 1, None)
            .expect("run fsm");
    }

    #[test]
    fn wait_for_quit_copies_error_on_ctrl_c() {
        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys([INTERRUPT_SENTINEL_KEY]);
        wait_for_quit(
            &terminal,
            Some("failed because the cosmos aligned the wrong way."),
        )
        .expect("wait should complete after copy shortcut");
        assert_eq!(
            terminal.clipboard_copies(),
            vec!["failed because the cosmos aligned the wrong way.".to_string()]
        );
    }

    #[test]
    fn wait_for_quit_does_not_copy_without_target() {
        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys([INTERRUPT_SENTINEL_KEY]);
        wait_for_quit(&terminal, None).expect("wait should complete even without copy target");
        assert!(terminal.clipboard_copies().is_empty());
    }

    #[test]
    fn wait_for_quit_copies_error_on_copy_shortcut() {
        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys(['c']);
        wait_for_quit(&terminal, Some("error line from agent"))
            .expect("wait should complete after copy shortcut");
        assert_eq!(
            terminal.clipboard_copies(),
            vec!["error line from agent".to_string()]
        );
    }

    #[test]
    fn wait_for_quit_copies_error_on_copy_shortcut_uppercase() {
        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys(['C']);
        wait_for_quit(&terminal, Some("error line from agent"))
            .expect("wait should complete after copy shortcut");
        assert_eq!(
            terminal.clipboard_copies(),
            vec!["error line from agent".to_string()]
        );
    }

    #[test]
    fn wait_for_quit_does_not_copy_error_on_other_key() {
        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys(['x']);
        wait_for_quit(&terminal, Some("error line from agent"))
            .expect("wait should complete after non-copy key");
        assert!(terminal.clipboard_copies().is_empty());
    }

    #[test]
    fn run_worker_pool_fsm_ignores_operator_hotkeys_by_default() {
        let dir = TempDir::new().expect("tempdir");
        let scope = test_scope(&dir);
        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        seed_task(&store, "hotkey actions task");

        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.orchestrator.parallelism = 1;
        cfg.quality_report.path = dir
            .path()
            .join(".gardener/quality.md")
            .display()
            .to_string();

        let terminal = FakeTerminal::new(true);
        terminal.enqueue_keys(['r', 'l', 'p', 'q']);
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(terminal.clone()),
        };

        let _ = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 2, None)
            .expect("run fsm");

        let lines = terminal.written_lines();
        assert!(!lines.iter().any(|line| line.contains("retry requested")));
        assert!(!lines
            .iter()
            .any(|line| line.contains("release-lease requested")));
        assert!(!lines
            .iter()
            .any(|line| line.contains("park/escalate requested")));

        let tasks = store.list_tasks().expect("list tasks");
        assert!(!tasks.iter().any(|task| {
            task.priority == Priority::P0 && task.title.contains("Escalation requested")
        }));
    }

    #[test]
    fn state_transition_guard_prevents_handoff_regression() {
        assert!(is_non_regressive_state_transition("handoff", "merging"));
        assert!(is_non_regressive_state_transition("handoff", "complete"));
        assert!(!is_non_regressive_state_transition("merging", "understand"));
        assert!(!is_non_regressive_state_transition(
            "complete",
            "understand"
        ));
    }

    #[test]
    fn apply_pool_stream_event_updates_doing_worker_from_live_events() {
        let mut workers = vec![WorkerRow {
            worker_id: "worker-1".to_string(),
            state: "claimed".to_string(),
            task_id: Some("task-1".to_string()),
            last_state_line: 0,
            task_title: "task one".to_string(),
            tool_line: "claimed".to_string(),
            breadcrumb: "state>claimed".to_string(),
            last_heartbeat_secs: 0,
            session_age_secs: 0,
            lease_held: true,
            session_missing: false,
            command_details: Vec::new(),
        }];
        let mut pulses = vec![Instant::now() - Duration::from_secs(10)];
        let before = pulses[0];

        let updated = apply_pool_stream_event(
            &mut workers,
            &mut pulses,
            PoolStreamEvent::StateChanged {
                slot_idx: 0,
                task_id: "task-1".to_string(),
                state: "doing".to_string(),
                details: "attempt=1".to_string(),
            },
        );
        assert!(updated);
        assert_eq!(workers[0].state, "doing");
        assert_eq!(workers[0].tool_line, "Doing (attempt=1)");
        assert_eq!(workers[0].breadcrumb, "state>doing");
        assert_eq!(
            workers[0].command_details.last().expect("command detail").1,
            "state doing: attempt=1"
        );
        assert!(pulses[0] > before);

        let tool_updated = apply_pool_stream_event(
            &mut workers,
            &mut pulses,
            PoolStreamEvent::ToolCommand {
                slot_idx: 0,
                task_id: "task-1".to_string(),
                command: "git status".to_string(),
            },
        );
        assert!(tool_updated);
        assert_eq!(workers[0].tool_line, "git status");
        assert_eq!(
            workers[0].command_details.last().expect("tool command").1,
            "git status"
        );

        let stale_task = apply_pool_stream_event(
            &mut workers,
            &mut pulses,
            PoolStreamEvent::StateChanged {
                slot_idx: 0,
                task_id: "task-2".to_string(),
                state: "complete".to_string(),
                details: String::new(),
            },
        );
        assert!(!stale_task);
        assert_eq!(workers[0].state, "doing");

        let regressive = apply_pool_stream_event(
            &mut workers,
            &mut pulses,
            PoolStreamEvent::StateChanged {
                slot_idx: 0,
                task_id: "task-1".to_string(),
                state: "claimed".to_string(),
                details: String::new(),
            },
        );
        assert!(!regressive);
    }

    #[test]
    fn run_worker_pool_limits_worker_slots_to_target() {
        let dir = TempDir::new().expect("tempdir");
        let scope = test_scope(&dir);
        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        seed_task(&store, "single-slot task");

        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.orchestrator.parallelism = 3;
        cfg.quality_report.path = dir
            .path()
            .join(".gardener/quality.md")
            .display()
            .to_string();

        let terminal = FakeTerminal::new(false);
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(terminal.clone()),
        };

        let _ = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 1, None)
            .expect("run fsm");
        let writes = terminal.written_lines();
        assert!(writes.iter().any(|line| line.contains("worker-1")));
        assert!(writes.iter().any(|line| line.contains("worker-2")));
        assert!(writes.iter().any(|line| line.contains("worker-3")));
        assert!(!writes.iter().any(|line| line.contains("worker-4")));
    }

    #[test]
    fn worker_execute_dispatch_includes_insert_awareness_metadata() {
        let dir = TempDir::new().expect("tempdir");
        let scope = test_scope(&dir);
        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        seed_task(&store, "dispatch-metadata task");

        let log_path = dir.path().join("otel-logs.jsonl");
        clear_run_logger();
        let _run_id = init_run_logger(&log_path, &scope.working_dir);

        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.orchestrator.parallelism = 1;
        cfg.quality_report.path = dir
            .path()
            .join(".gardener/quality.md")
            .display()
            .to_string();

        let terminal = FakeTerminal::new(false);
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(terminal.clone()),
        };
        let completed = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 1, None)
            .expect("run fsm");
        assert_eq!(completed, 1);

        let events = std::fs::read_to_string(&log_path).expect("read logs");
        let dispatch_event = events
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|entry| {
                entry.get("event_type").and_then(|v| v.as_str()) == Some("worker.execute.dispatch")
            })
            .expect("found execute dispatch log event");
        let payload = dispatch_event
            .get("payload")
            .and_then(|v| v.as_object())
            .expect("dispatch payload object");

        assert!(payload.contains_key("task_created_at"));
        assert!(payload.contains_key("task_last_updated"));
        assert!(payload.contains_key("run_started_at_ms"));
        assert!(payload.contains_key("task_age_ms"));
        assert!(payload.contains_key("inserted_after_run_start"));
        assert!(payload
            .get("task_age_ms")
            .and_then(|value| value.as_i64())
            .is_some());
        assert!(payload
            .get("inserted_after_run_start")
            .and_then(|value| value.as_bool())
            .is_some());
        clear_run_logger();
    }

    #[test]
    fn handle_merge_summary_rejects_false_complete_transition() {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        let row = store
            .upsert_task(NewTask {
                kind: TaskKind::Maintenance,
                title: "merge rejected transition".to_string(),
                details: "force mark_complete rejection".to_string(),
                scope_key: "scope".to_string(),
                rationale: "worker_pool regression".to_string(),
                priority: Priority::P1,
                source: "test".to_string(),
                related_pr: Some(321),
                related_branch: Some("gardener/rejected-transition".to_string()),
            })
            .expect("seed task");

        let mut workers = vec![WorkerRow {
            worker_id: "merge-worker".to_string(),
            state: "merging".to_string(),
            task_id: Some(row.task_id.clone()),
            last_state_line: 0,
            task_title: "merge rejected transition".to_string(),
            tool_line: "merging PR #321".to_string(),
            breadcrumb: "merging".to_string(),
            last_heartbeat_secs: 0,
            session_age_secs: 0,
            lease_held: true,
            session_missing: false,
            command_details: Vec::new(),
        }];
        let summary = crate::worker::WorkerRunSummary {
            worker_id: "merge-worker".to_string(),
            session_id: "session-1".to_string(),
            final_state: WorkerState::Complete,
            logs: Vec::new(),
            teardown: None,
            failure_reason: None,
        };
        let mut completed = 0usize;
        let mut merged = 0usize;
        let mut failed = 0usize;

        let handling = handle_merge_summary(
            &store,
            &mut workers,
            0,
            7,
            &row.task_id,
            &summary,
            &mut completed,
            &mut merged,
            &mut failed,
        )
        .expect("handle summary");

        assert_eq!(handling, MergeSummaryHandling::EarlyContinue);
        assert_eq!(completed, 0);
        assert_eq!(merged, 0);
        assert_eq!(failed, 1);
        assert_eq!(workers[0].state, "failed");
        assert_eq!(workers[0].last_state_line, 7);
        assert_eq!(workers[0].task_id.as_deref(), Some(row.task_id.as_str()));
        assert!(workers[0]
            .tool_line
            .contains("merge complete transition rejected"));
        assert_eq!(workers[0].breadcrumb, "failed");
        assert!(!workers[0].lease_held);
        let task = store
            .get_task(&row.task_id)
            .expect("fetch task")
            .expect("task exists");
        assert_ne!(task.status, TaskStatus::Complete);
    }

    #[test]
    fn available_doing_slots_respects_in_flight_merge_budget() {
        let slots = available_doing_slots(4, 1, 0, 1);
        assert_eq!(slots, 0);
    }

    #[test]
    fn handle_doing_complete_transition_rejects_false_complete() {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        let row = store
            .upsert_task(NewTask {
                kind: TaskKind::Maintenance,
                title: "doing complete rejection".to_string(),
                details: "force mark_complete reject".to_string(),
                scope_key: "scope".to_string(),
                rationale: "fsm invariant test".to_string(),
                priority: Priority::P1,
                source: "test".to_string(),
                related_pr: None,
                related_branch: None,
            })
            .expect("seed task");
        let mut workers = vec![WorkerRow {
            worker_id: "worker-1".to_string(),
            state: "doing".to_string(),
            task_id: Some(row.task_id.clone()),
            last_state_line: 0,
            task_title: "doing complete rejection".to_string(),
            tool_line: "doing".to_string(),
            breadcrumb: "state>doing".to_string(),
            last_heartbeat_secs: 0,
            session_age_secs: 0,
            lease_held: true,
            session_missing: false,
            command_details: Vec::new(),
        }];
        let mut completed = 0usize;
        let mut failed = 0usize;

        let handling = handle_doing_complete_transition(
            &store,
            &mut workers,
            0,
            11,
            "worker-1",
            &row.task_id,
            &mut completed,
            &mut failed,
        )
        .expect("transition");

        assert_eq!(handling, DoingSummaryHandling::ContinueLoop);
        assert_eq!(completed, 0);
        assert_eq!(failed, 1);
        assert_ne!(workers[0].state, "complete");
    }

    #[test]
    fn handle_doing_non_complete_transition_parks_to_unresolved() {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");
        let row = store
            .upsert_task(NewTask {
                kind: TaskKind::Maintenance,
                title: "parked transition".to_string(),
                details: "parked tasks should be unresolved".to_string(),
                scope_key: "scope".to_string(),
                rationale: "fsm invariant test".to_string(),
                priority: Priority::P1,
                source: "test".to_string(),
                related_pr: None,
                related_branch: None,
            })
            .expect("seed task");
        let _ = store.claim_next("worker-1", 60).expect("claim");
        assert!(store
            .mark_in_progress(&row.task_id, "worker-1")
            .expect("mark in progress"));
        let mut workers = vec![WorkerRow {
            worker_id: "worker-1".to_string(),
            state: "parked".to_string(),
            task_id: Some(row.task_id.clone()),
            last_state_line: 0,
            task_title: "parked transition".to_string(),
            tool_line: "parked".to_string(),
            breadcrumb: "state>parked".to_string(),
            last_heartbeat_secs: 0,
            session_age_secs: 0,
            lease_held: true,
            session_missing: false,
            command_details: Vec::new(),
        }];
        let mut failed = 0usize;
        let summary = crate::worker::WorkerRunSummary {
            worker_id: "worker-1".to_string(),
            session_id: "session-parked".to_string(),
            final_state: WorkerState::Parked,
            logs: Vec::new(),
            teardown: None,
            failure_reason: Some("review requested changes".to_string()),
        };

        let handling = handle_doing_non_complete_transition(
            &store,
            &mut workers,
            0,
            12,
            "worker-1",
            &row.task_id,
            &summary,
            &mut failed,
        )
        .expect("transition");

        assert_eq!(handling, DoingSummaryHandling::ContinueLoop);
        assert_eq!(failed, 1);
        assert_eq!(workers[0].state, "unresolved");
        let task = store
            .get_task(&row.task_id)
            .expect("fetch task")
            .expect("task exists");
        assert_eq!(task.status, TaskStatus::Unresolved);
    }
}
