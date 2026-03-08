use super::dashboard::{dashboard_snapshot, render};
use super::event_handling::drain_pool_events;
use super::hotkeys::{handle_hotkeys, HotkeyState};
use super::scheduling::{
    append_worker_command, claim_task_for_worker_slot, claim_tasks_for_available_workers,
    execution_task_packet, mark_merge_worker_busy, maybe_start_merge, refresh_worker_heartbeats,
    set_worker_idle,
};
use super::{
    DoingSummaryHandling, MergeSummaryHandling, PoolResultMessage, PoolStreamEvent,
    IDLE_WORKER_POLL_ATTEMPTS, IDLE_WORKER_POLL_DELAY_MS, MERGE_WORKER_ID, WORKER_POOL_ID,
};
use crate::backlog_store::{BacklogStore, BacklogTask};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::runtime::{request_interrupt, ProductionRuntime, Terminal};
use crate::tui::WorkerRow;
use crate::types::RuntimeScope;
use crate::worker::{
    clear_state_sink, execute_merge_phase, execute_task, install_state_sink, MergeRequest,
    WorkerOutcome, WorkerRunSummary, WorkerStreamEvent,
};
use crate::worktree::WorktreeClient;
use serde_json::json;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(super) struct EpochOutcome {
    pub(super) shutdown_error: Option<(String, String, String)>,
    pub(super) quit_requested: bool,
    pub(super) exhausted_backlog: bool,
}

#[allow(clippy::too_many_arguments)]
fn spawn_doing_worker<'scope, 'env>(
    scope_guard: &'scope std::thread::Scope<'scope, 'env>,
    tx: mpsc::Sender<PoolResultMessage>,
    event_tx: mpsc::Sender<PoolStreamEvent>,
    worker_id: String,
    idx: usize,
    task: BacklogTask,
    task_override: Option<&'env str>,
    cfg: &'env AppConfig,
    runtime: &'env ProductionRuntime,
    runtime_scope: &'env RuntimeScope,
    run_started_at_ms: i64,
) {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn run_pool_epoch(
    runtime: &ProductionRuntime,
    runtime_scope: &RuntimeScope,
    cfg: &AppConfig,
    store: &BacklogStore,
    terminal: &dyn Terminal,
    workers: &mut [WorkerRow],
    operator_hotkeys: bool,
    report_visible: &mut bool,
    report_content: &mut Option<String>,
    merge_row_idx: usize,
    last_worker_state_line: &mut usize,
    event_sequence: &mut usize,
    last_activity_pulse: &mut [Instant],
    parallelism: usize,
    target: usize,
    task_override: Option<&str>,
    run_started_at_ms: i64,
    repo_root: &Path,
    worktree_client: &WorktreeClient<'_>,
    completed: &mut usize,
    merged: &mut usize,
    failed: &mut usize,
    hb: u64,
    lt: u64,
) -> Result<EpochOutcome, GardenerError> {
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
    let (merge_tx, merge_rx): (mpsc::Sender<MergeRequest>, mpsc::Receiver<MergeRequest>) =
        mpsc::channel();
    let mut merge_tx = Some(merge_tx);
    let mut claimed_any = false;

    if let Some((pr_number, task_id, task_summary)) = maybe_start_merge(
        &mut active_merging,
        &mut merge_tx,
        store,
        repo_root,
        worktree_client,
    )? {
        claimed_any = true;
        mark_merge_worker_busy(
            workers,
            last_activity_pulse,
            merge_row_idx,
            pr_number,
            &task_id,
            *last_worker_state_line,
            &task_summary,
        );
    }

    claimed_any = claim_tasks_for_available_workers(
        workers,
        &mut claimed,
        *completed,
        active_merging,
        *last_worker_state_line,
        last_activity_pulse,
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
                workers,
                &mut claimed,
                *completed,
                active_merging,
                *last_worker_state_line,
                last_activity_pulse,
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
            return Ok(EpochOutcome {
                shutdown_error: None,
                quit_requested: false,
                exhausted_backlog: true,
            });
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
        for (idx, task) in claimed {
            let worker_id = workers[idx].worker_id.clone();
            spawn_doing_worker(
                scope_guard,
                tx.clone(),
                event_tx.clone(),
                worker_id,
                idx,
                task,
                task_override,
                cfg,
                runtime,
                runtime_scope,
                run_started_at_ms,
            );
        }

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
                    let _ = merge_result_tx.send(PoolResultMessage::MergeResult { task_id, result });
                }
            });
        }

        let mut merge_tx = merge_tx;

        while active_doing > 0 || active_merging > 0 {
            let mut updated =
                drain_pool_events(workers, last_activity_pulse, event_sequence, &event_rx);
            if handle_hotkeys(&mut HotkeyState {
                runtime,
                scope: runtime_scope,
                cfg,
                store,
                workers,
                operator_hotkeys,
                terminal,
                report_visible,
                report_content,
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
                                shutdown_error = Some((worker_id.clone(), task_id.clone(), msg));
                                request_interrupt();
                            }
                            Ok(WorkerOutcome::HandoffToMerge(req)) => {
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
                                    *failed = failed.saturating_add(1);
                                    workers[idx].state = "failed".to_string();
                                    workers[idx].task_id = None;
                                    workers[idx].last_state_line = *last_worker_state_line;
                                    let fail_msg =
                                        format!("handoff failed (transition rejected) {}", task_id);
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
                                for event in &req.logs {
                                    workers[idx].state = event.state.as_str().to_string();
                                    let prompt = format!("prompt {}", event.prompt_version);
                                    workers[idx].tool_line = prompt.clone();
                                    append_worker_command(&mut workers[idx], &prompt);
                                    workers[idx].breadcrumb =
                                        format!("state>{}", event.state.as_str());
                                    last_activity_pulse[idx] = Instant::now();
                                }
                                workers[idx].state = "merging".to_string();
                                let handoff_msg =
                                    format!("handed off PR #{} to merge worker", req.pr_number);
                                workers[idx].tool_line = handoff_msg.clone();
                                append_worker_command(&mut workers[idx], &handoff_msg);
                                workers[idx].breadcrumb = "handoff>merge".to_string();
                                workers[idx].lease_held = false;
                                workers[idx].last_state_line = *last_worker_state_line;
                                if let Some((pr_number, task_id, task_summary)) = maybe_start_merge(
                                    &mut active_merging,
                                    &mut merge_tx,
                                    store,
                                    repo_root,
                                    worktree_client,
                                )? {
                                    mark_merge_worker_busy(
                                        workers,
                                        last_activity_pulse,
                                        merge_row_idx,
                                        pr_number,
                                        &task_id,
                                        *last_worker_state_line,
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
                                process_completed_summary(
                                    store,
                                    terminal,
                                    workers,
                                    idx,
                                    &worker_id,
                                    &task_id,
                                    &summary,
                                    last_activity_pulse,
                                    last_worker_state_line,
                                    completed,
                                    failed,
                                    hb,
                                    lt,
                                )?;
                            }
                        }
                    }

                    if shutdown_error.is_none()
                        && !quit_requested
                        && completed
                            .saturating_add(active_doing)
                            .saturating_add(active_merging)
                            < target
                    {
                        if let Some(task) = claim_task_for_worker_slot(
                            workers,
                            idx,
                            *last_worker_state_line,
                            last_activity_pulse,
                            run_started_at_ms,
                            store,
                            cfg,
                            terminal,
                            hb,
                            lt,
                        )? {
                            last_render_completed = Some(Instant::now());
                            let worker_id = workers[idx].worker_id.clone();
                            spawn_doing_worker(
                                scope_guard,
                                tx.clone(),
                                event_tx.clone(),
                                worker_id,
                                idx,
                                task,
                                task_override,
                                cfg,
                                runtime,
                                runtime_scope,
                                run_started_at_ms,
                            );
                            active_doing = active_doing.saturating_add(1);
                        }
                    }
                    if active_doing == 0 && active_merging == 0 {
                        merge_tx.take();
                    }
                    refresh_worker_heartbeats(workers, last_activity_pulse);
                    render(terminal, workers, &dashboard_snapshot(store)?, hb, lt)?;
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
                                append_worker_command(&mut workers[merge_row_idx], &interrupted_msg);
                                workers[merge_row_idx].last_state_line = *last_worker_state_line;
                            }
                            Err(err) => {
                                let msg = err.to_string();
                                *failed = failed.saturating_add(1);
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
                                let fail_msg =
                                    format!("merge failed: {}", msg.chars().take(100).collect::<String>());
                                workers[merge_row_idx].tool_line = fail_msg.clone();
                                append_worker_command(&mut workers[merge_row_idx], &fail_msg);
                                workers[merge_row_idx].last_state_line = *last_worker_state_line;
                            }
                            Ok(summary) => {
                                let handling = handle_merge_summary(
                                    store,
                                    workers,
                                    merge_row_idx,
                                    *last_worker_state_line,
                                    &task_id,
                                    &summary,
                                    completed,
                                    merged,
                                    failed,
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
                        worktree_client,
                    )? {
                        mark_merge_worker_busy(
                            workers,
                            last_activity_pulse,
                            merge_row_idx,
                            pr_number,
                            &task_id,
                            *last_worker_state_line,
                            &task_summary,
                        );
                    }
                    if active_merging == 0 {
                        set_worker_idle(&mut workers[merge_row_idx], "waiting for merge");
                        workers[merge_row_idx].task_id = None;
                    }
                    refresh_worker_heartbeats(workers, last_activity_pulse);
                    render(terminal, workers, &dashboard_snapshot(store)?, hb, lt)?;
                    last_render_completed = Some(Instant::now());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if drain_pool_events(workers, last_activity_pulse, event_sequence, &event_rx) {
                        updated = true;
                    }
                    if *last_worker_state_line < *event_sequence {
                        *last_worker_state_line = *event_sequence;
                    }
                    if updated || last_dashboard_refresh.elapsed() >= Duration::from_secs(1) {
                        refresh_worker_heartbeats(workers, last_activity_pulse);
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
                        render(terminal, workers, &snapshot, hb, lt)?;
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

    Ok(EpochOutcome {
        shutdown_error,
        quit_requested,
        exhausted_backlog: false,
    })
}

#[allow(clippy::too_many_arguments)]
fn process_completed_summary(
    store: &BacklogStore,
    terminal: &dyn Terminal,
    workers: &mut [WorkerRow],
    idx: usize,
    worker_id: &str,
    task_id: &str,
    summary: &WorkerRunSummary,
    last_activity_pulse: &mut [Instant],
    last_worker_state_line: &usize,
    completed: &mut usize,
    failed: &mut usize,
    hb: u64,
    lt: u64,
) -> Result<(), GardenerError> {
    for event in &summary.logs {
        workers[idx].state = event.state.as_str().to_string();
        let prompt = format!("prompt {}", event.prompt_version);
        workers[idx].tool_line = prompt.clone();
        append_worker_command(&mut workers[idx], &prompt);
        workers[idx].breadcrumb = format!("state>{}", event.state.as_str());
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
        refresh_worker_heartbeats(workers, last_activity_pulse);
        render(terminal, workers, &dashboard_snapshot(store)?, hb, lt)?;
    }
    if summary.final_state == crate::types::WorkerState::Complete {
        let _ = handle_doing_complete_transition(
            store,
            workers,
            idx,
            *last_worker_state_line,
            worker_id,
            task_id,
            completed,
            failed,
        )?;
    } else {
        let _ = handle_doing_non_complete_transition(
            store,
            workers,
            idx,
            *last_worker_state_line,
            worker_id,
            task_id,
            summary,
            failed,
        )?;
    }
    Ok(())
}

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
