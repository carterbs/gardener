use crate::backlog_store::BacklogStore;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::hotkeys::{
    action_for_key_with_mode, operator_hotkeys_enabled, HotkeyAction as AppHotkeyAction,
};
use crate::logging::{
    append_run_log, current_log_line_count, recent_worker_log_lines, recent_worker_state_events,
    recent_worker_tool_commands, structured_fallback_line,
};
use crate::priority::Priority;
use crate::runtime::Terminal;
use crate::runtime::{
    clear_interrupt, request_interrupt, ProductionRuntime, INTERRUPT_SENTINEL_KEY,
};
use crate::startup::refresh_quality_report;
use crate::task_identity::TaskKind;
use crate::tui::{
    format_state_label, reset_workers_scroll, scroll_workers_down, scroll_workers_up, BacklogView,
    QueueStats, WorkerRow,
};
use crate::types::RuntimeScope;
use crate::worker::{
    execute_merge_phase, execute_task, worktree_branch_for, worktree_path_for, MergeRequest,
    WorkerOutcome,
};
use crate::worker_identity::WorkerIdentity;
use crate::worktree::WorktreeClient;
use serde_json::json;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const WORKER_POOL_ID: &str = "worker_pool";
const MERGE_WORKER_ID: &str = "merge-worker";
const WORKER_COMMAND_HISTORY_LIMIT: usize = 32;
const COPY_SHORTCUT_KEY: char = 'c';
const IDLE_WORKER_POLL_DELAY_MS: u64 = 250;
const IDLE_WORKER_POLL_ATTEMPTS: usize = 4;

/// Messages on the result channel. Doing workers send `DoingResult`, the merge
/// worker sends `MergeResult`.
enum PoolResultMessage {
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

struct HotkeyState<'a> {
    runtime: &'a ProductionRuntime,
    scope: &'a RuntimeScope,
    cfg: &'a AppConfig,
    store: &'a BacklogStore,
    workers: &'a mut [WorkerRow],
    operator_hotkeys: bool,
    terminal: &'a dyn Terminal,
    report_visible: &'a mut bool,
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
    let mut last_worker_command_line = current_log_line_count();
    let mut last_worker_state_line = last_worker_command_line;
    let mut last_activity_pulse = vec![Instant::now(); workers.len()];
    let command_poll_chunk = 32;
    let mut completed = 0usize;
    let run_started_at_ms = now_unix_millis();

    let claim_tasks_for_available_workers =
        |workers: &mut [WorkerRow],
         claimed: &mut Vec<(usize, crate::backlog_store::BacklogTask)>,
         completed: usize,
         last_worker_state_line: usize,
         last_activity_pulse: &mut Vec<Instant>|
         -> Result<bool, GardenerError> {
            let mut claimed_any = false;
            claimed.clear();
            let available_slots = parallelism.min(target.saturating_sub(completed));
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
                let _ = store.mark_in_progress(&task.task_id, &worker_id)?;
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
        };

    let mark_merge_worker_busy = |workers: &mut [WorkerRow],
                                  last_activity_pulse: &mut Vec<Instant>,
                                  merge_row_idx: usize,
                                  pr_number: u64,
                                  task_id: &str,
                                  last_worker_state_line: usize,
                                  task_summary: &str| {
        workers[merge_row_idx].state = "merging".to_string();
        workers[merge_row_idx].task_id = Some(task_id.to_string());
        workers[merge_row_idx].last_state_line = last_worker_state_line;
        workers[merge_row_idx].task_title =
            format!("PR #{pr_number} {task_title}", task_title = task_summary);
        let merge_msg = format!("merging PR #{pr_number}");
        workers[merge_row_idx].tool_line = merge_msg.clone();
        append_worker_command(&mut workers[merge_row_idx], &merge_msg);
        workers[merge_row_idx].breadcrumb = "merging".to_string();
        workers[merge_row_idx].lease_held = true;
        last_activity_pulse[merge_row_idx] = Instant::now();
    };
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
        })? {
            return Ok(completed);
        }
        if report_visible {
            continue;
        }
        let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);
        let worktree_client = WorktreeClient::new(runtime.process_runner.as_ref(), repo_root);
        let maybe_start_merge = |active_merging: &mut usize,
                                 merge_tx: &mut Option<mpsc::Sender<MergeRequest>>|
         -> Result<Option<(u64, String, String)>, GardenerError> {
            if *active_merging >= 1 {
                return Ok(None);
            }
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
                    let _ = store.release_lease(&task.task_id, MERGE_WORKER_ID);
                    return Ok(None);
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
                let _ = store.release_lease(&task.task_id, MERGE_WORKER_ID);
                return Ok(None);
            }
            if let Some(mtx) = merge_tx {
                let identity = WorkerIdentity::new(MERGE_WORKER_ID);
                let task_summary = task.title;
                let _ = mtx.send(MergeRequest {
                    slot_idx: 0,
                    task_id: task.task_id,
                    task_summary: task_summary.clone(),
                    attempt_count: task.attempt_count,
                    worker_id: identity.worker_id,
                    session_id: identity.session.session_id,
                    worktree_path,
                    branch,
                    pr_number,
                    logs: Vec::new(),
                    handoff_evidence_bundle: None,
                });
                *active_merging = active_merging.saturating_add(1);
                return Ok(Some((pr_number, task_id, task_summary)));
            }
            Ok(None)
        };
        let mut claimed = Vec::new();
        let mut claimed_any = claim_tasks_for_available_workers(
            &mut workers,
            &mut claimed,
            completed,
            last_worker_state_line,
            &mut last_activity_pulse,
        )?;

        let mut active_merging = 0usize;
        let (tx, rx): (
            mpsc::Sender<PoolResultMessage>,
            mpsc::Receiver<PoolResultMessage>,
        ) = mpsc::channel();
        // Merge queue: doing workers send MergeRequests here.
        let (merge_tx, merge_rx): (mpsc::Sender<MergeRequest>, mpsc::Receiver<MergeRequest>) =
            mpsc::channel();
        let runtime_scope = scope.clone();
        let mut merge_tx = Some(merge_tx);
        if let Some((pr_number, task_id, task_summary)) =
            maybe_start_merge(&mut active_merging, &mut merge_tx)?
        {
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

        if !claimed_any && active_merging == 0 {
            let mut claimed_on_idle = false;
            for _ in 0..IDLE_WORKER_POLL_ATTEMPTS {
                std::thread::sleep(Duration::from_millis(IDLE_WORKER_POLL_DELAY_MS));
                claimed_any = claim_tasks_for_available_workers(
                    &mut workers,
                    &mut claimed,
                    completed,
                    last_worker_state_line,
                    &mut last_activity_pulse,
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
                let worker_id = workers[idx].worker_id.clone();
                let task_id = task.task_id.clone();
                let task_summary = task_override.unwrap_or(task.title.as_str()).to_string();
                let attempt_count = task.attempt_count;
                let task_created_at = task.created_at;
                let task_last_updated = task.last_updated;
                let cfg = cfg.clone();
                let process_runner = runtime.process_runner.clone();
                let worker_scope = runtime_scope.clone();
                scope_guard.spawn(move || {
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
                    );
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
                scope_guard.spawn(move || {
                    while let Ok(req) = merge_rx.recv() {
                        let task_id = req.task_id.clone();
                        let result = execute_merge_phase(
                            &req,
                            &merge_cfg,
                            merge_runner.as_ref(),
                            merge_file_system.as_ref(),
                            merge_clock.as_ref(),
                            &merge_scope,
                        );
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
                if handle_hotkeys(&mut HotkeyState {
                    runtime,
                    scope: &runtime_scope,
                    cfg,
                    store,
                    workers: &mut workers,
                    operator_hotkeys,
                    terminal,
                    report_visible: &mut report_visible,
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
                                    let _ = store.mark_merge_pending(&task_id, &worker_id);
                                    let _ = store.set_related_pr(
                                        &task_id,
                                        req.pr_number as i64,
                                        &req.branch,
                                    );
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
                                        maybe_start_merge(&mut active_merging, &mut merge_tx)?
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
                                        let _ = store.mark_complete(&task_id, &worker_id)?;
                                        completed = completed.saturating_add(1);
                                        workers[idx].state = "complete".to_string();
                                        workers[idx].task_id = None;
                                        workers[idx].last_state_line = last_worker_state_line;
                                        let completed_message = format!("completed {}", task_id);
                                        workers[idx].tool_line = completed_message.clone();
                                        append_worker_command(
                                            &mut workers[idx],
                                            &completed_message,
                                        );
                                        workers[idx].breadcrumb = "complete".to_string();
                                        workers[idx].lease_held = false;
                                        append_run_log(
                                            "info",
                                            "worker.task.completed",
                                            json!({
                                                "worker_id": worker_id,
                                                "task_id": task_id,
                                                "completed": completed
                                            }),
                                        );
                                    } else {
                                        workers[idx].state = "failed".to_string();
                                        workers[idx].task_id = None;
                                        workers[idx].last_state_line = last_worker_state_line;
                                        let failed_message = if let Some(reason) =
                                            summary.failure_reason.clone()
                                        {
                                            if reason.is_empty() {
                                                format!("failed {}", task_id)
                                            } else {
                                                let truncated =
                                                    reason.chars().take(150).collect::<String>();
                                                if reason.chars().count() > 150 {
                                                    format!("failed: {}…", truncated)
                                                } else {
                                                    format!("failed: {}", reason)
                                                }
                                            }
                                        } else {
                                            format!("failed {}", task_id)
                                        };
                                        workers[idx].tool_line = failed_message.clone();
                                        append_worker_command(&mut workers[idx], &failed_message);
                                        workers[idx].breadcrumb = "failed".to_string();
                                        workers[idx].lease_held = false;
                                        append_run_log(
                                            "error",
                                            "worker.task.failed",
                                            json!({
                                                "worker_id": worker_id,
                                                "task_id": task_id,
                                                "final_state": summary.final_state.as_str()
                                            }),
                                        );
                                        if summary.final_state == crate::types::WorkerState::Failed
                                        {
                                            let unresolved =
                                                store.mark_unresolved(&task_id, &worker_id)?;
                                            append_run_log(
                                                "warn",
                                                "worker.task.unresolved",
                                                json!({
                                                    "worker_id": worker_id,
                                                    "task_id": task_id,
                                                    "marked_unresolved": unresolved,
                                                    "failure_reason": summary.failure_reason,
                                                }),
                                            );
                                            let unresolved_message = if unresolved {
                                                format!("unresolved {}", task_id)
                                            } else {
                                                failed_message.clone()
                                            };
                                            workers[idx].state = "unresolved".to_string();
                                            workers[idx].last_state_line = last_worker_state_line;
                                            workers[idx].tool_line = unresolved_message.clone();
                                            workers[idx].breadcrumb = "unresolved".to_string();
                                            append_worker_command(
                                                &mut workers[idx],
                                                &unresolved_message,
                                            );
                                        } else {
                                            workers[idx].task_id = None;
                                            workers[idx].last_state_line = last_worker_state_line;
                                            let _ = store.release_lease(&task_id, &worker_id)?;
                                        }
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
                                let _ = store.mark_in_progress(&task.task_id, &worker_id)?;

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
                                let task_summary =
                                    task_override.unwrap_or(task.title.as_str()).to_string();
                                let attempt_count = task.attempt_count;
                                let task_created_at = task.created_at;
                                let task_last_updated = task.last_updated;
                                let cfg = cfg.clone();
                                let process_runner = runtime.process_runner.clone();
                                let worker_scope = runtime_scope.clone();
                                scope_guard.spawn(move || {
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
                                    );
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
                                    if summary.final_state == crate::types::WorkerState::Complete {
                                        let _ = store.mark_complete(&task_id, MERGE_WORKER_ID)?;
                                        completed = completed.saturating_add(1);
                                        workers[merge_row_idx].state = "complete".to_string();
                                        workers[merge_row_idx].task_id = Some(task_id.clone());
                                        workers[merge_row_idx].last_state_line =
                                            last_worker_state_line;
                                        let done_msg = format!("merged {}", task_id);
                                        workers[merge_row_idx].tool_line = done_msg.clone();
                                        append_worker_command(
                                            &mut workers[merge_row_idx],
                                            &done_msg,
                                        );
                                        workers[merge_row_idx].breadcrumb = "complete".to_string();
                                        workers[merge_row_idx].lease_held = false;
                                        append_run_log(
                                            "info",
                                            "merge_worker.task.completed",
                                            json!({
                                                "worker_id": MERGE_WORKER_ID,
                                                "task_id": task_id,
                                                "completed": completed
                                            }),
                                        );
                                    } else {
                                        let _ = store.mark_unresolved(&task_id, MERGE_WORKER_ID)?;
                                        workers[merge_row_idx].state = "failed".to_string();
                                        workers[merge_row_idx].task_id = Some(task_id.clone());
                                        workers[merge_row_idx].last_state_line =
                                            last_worker_state_line;
                                        let fail_msg = summary
                                            .failure_reason
                                            .as_deref()
                                            .unwrap_or("merge failed");
                                        let truncated =
                                            fail_msg.chars().take(100).collect::<String>();
                                        workers[merge_row_idx].tool_line = truncated.clone();
                                        append_worker_command(
                                            &mut workers[merge_row_idx],
                                            &truncated,
                                        );
                                        workers[merge_row_idx].breadcrumb = "failed".to_string();
                                        workers[merge_row_idx].lease_held = false;
                                    }
                                }
                            }
                        }
                        if let Some((pr_number, task_id, task_summary)) =
                            maybe_start_merge(&mut active_merging, &mut merge_tx)?
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
                        let updated_commands = append_worker_tool_commands(
                            &mut workers,
                            &mut last_worker_command_line,
                            command_poll_chunk,
                        );
                        let updated_states = append_worker_state_events(
                            &mut workers,
                            &mut last_worker_state_line,
                            command_poll_chunk,
                        );
                        if updated_commands
                            || updated_states
                            || last_dashboard_refresh.elapsed() >= Duration::from_secs(1)
                        {
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

        if let Some((worker_id, task_id, reason)) = shutdown_error {
            let shutdown_message = worker_failure_prompt(&worker_id, &task_id, &reason);
            if terminal.stdin_is_tty() {
                terminal.draw_shutdown_screen("Error", &shutdown_message)?;
            } else {
                terminal.write_line(&format!(
                    "Error: worker {worker_id} task {task_id}: {reason}"
                ))?;
            }
            wait_for_quit(terminal, Some(&shutdown_message))?;
            return Ok(completed);
        }
        if quit_requested {
            return Ok(completed);
        }
    }
    append_run_log(
        "info",
        "worker_pool.completed",
        json!({
            "worker_id": WORKER_POOL_ID,
            "completed": completed,
            "target": target
        }),
    );
    let shutdown_title = if completed >= target {
        "All Tasks Complete".to_string()
    } else {
        "No More Work".to_string()
    };
    let shutdown_message = if completed >= target {
        format!("Completed {completed} of {target} task(s).")
    } else if completed == 0 {
        "No tasks were available in the backlog.".to_string()
    } else {
        format!("Completed {completed} task(s). Backlog is empty.")
    };
    if terminal.stdin_is_tty() {
        terminal.draw_shutdown_screen(&shutdown_title, &shutdown_message)?;
    } else {
        terminal.write_line(&format!("{shutdown_title}: {shutdown_message}"))?;
    }
    wait_for_quit(terminal, None)?;
    Ok(completed)
}

fn wait_for_quit(terminal: &dyn Terminal, copy_target: Option<&str>) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "worker_pool.wait_for_quit.started",
        json!({
            "worker_id": WORKER_POOL_ID,
            "has_tty": terminal.stdin_is_tty(),
        }),
    );
    if !terminal.stdin_is_tty() {
        return Ok(());
    }
    // Poll until the user presses any key. poll_key returns None on timeout,
    // Some(key) when a key arrives. For fake terminals with an empty queue
    // the first poll immediately returns None and we exit — this prevents
    // test hangs while still blocking on a real TTY until input is received.
    loop {
        match terminal.poll_key(100)? {
            Some(INTERRUPT_SENTINEL_KEY) => {
                if let Some(target) = copy_target {
                    if let Err(error) = terminal.copy_to_clipboard(target) {
                        append_run_log(
                            "warn",
                            "worker_pool.error_copy.failed",
                            json!({
                                "worker_id": WORKER_POOL_ID,
                                "error": error.to_string()
                            }),
                        );
                    } else {
                        append_run_log(
                            "info",
                            "worker_pool.error_copy.success",
                            json!({
                                "worker_id": WORKER_POOL_ID
                            }),
                        );
                    }
                }
                return Ok(());
            }
            Some(key) if is_copy_shortcut_key(key) && copy_target.is_some() => {
                if let Some(target) = copy_target {
                    if let Err(error) = terminal.copy_to_clipboard(target) {
                        append_run_log(
                            "warn",
                            "worker_pool.error_copy.failed",
                            json!({
                                "worker_id": WORKER_POOL_ID,
                                "error": error.to_string()
                            }),
                        );
                    } else {
                        append_run_log(
                            "info",
                            "worker_pool.error_copy.success",
                            json!({
                                "worker_id": WORKER_POOL_ID
                            }),
                        );
                    }
                }
                return Ok(());
            }
            Some(_) => return Ok(()),
            None => {
                // On a real terminal the key listener is running; keep waiting.
                // In test mode (FakeTerminal) None means queue exhausted: exit.
                if !crate::runtime::KEY_LISTENER_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(());
                }
            }
        }
    }
}

fn handle_hotkeys(state: &mut HotkeyState<'_>) -> Result<bool, GardenerError> {
    let runtime = state.runtime;
    let scope = state.scope;
    let cfg = state.cfg;
    let store = state.store;
    let workers = &mut *state.workers;
    let operator_hotkeys = state.operator_hotkeys;
    let terminal = state.terminal;
    let report_visible = &mut *state.report_visible;

    if !terminal.stdin_is_tty() {
        return Ok(false);
    }
    let mut redraw_dashboard = false;
    if let Some(key) = terminal.poll_key(10)? {
        if key == '\0' {
            redraw_dashboard = true;
        }
        if key == INTERRUPT_SENTINEL_KEY {
            append_run_log(
                "warn",
                "hotkey.quit",
                json!({
                    "worker_id": WORKER_POOL_ID
                }),
            );
            request_interrupt();
            return Ok(true);
        }
        match hotkey_action(key, operator_hotkeys) {
            Some(AppHotkeyAction::Quit) => {
                append_run_log(
                    "warn",
                    "hotkey.quit",
                    json!({
                        "worker_id": WORKER_POOL_ID
                    }),
                );
                request_interrupt();
                return Ok(true);
            }
            Some(AppHotkeyAction::ScrollDown) => {
                redraw_dashboard = scroll_workers_down();
            }
            Some(AppHotkeyAction::ScrollUp) => {
                redraw_dashboard = scroll_workers_up();
            }
            Some(AppHotkeyAction::Retry) => {
                let released = store.recover_stale_leases(now_unix_millis())?;
                append_run_log(
                    "info",
                    "hotkey.retry",
                    json!({
                        "worker_id": WORKER_POOL_ID,
                        "released": released
                    }),
                );
                terminal.write_line(&format!(
                    "retry requested: released {released} stale lease(s)"
                ))?;
                redraw_dashboard = true;
            }
            Some(AppHotkeyAction::ReleaseLease) => {
                let release_now = now_unix_millis()
                    .saturating_add((cfg.scheduler.lease_timeout_seconds as i64 + 1) * 1000);
                let released = store.recover_stale_leases(release_now)?;
                append_run_log(
                    "info",
                    "hotkey.release_lease",
                    json!({
                        "worker_id": WORKER_POOL_ID,
                        "released": released
                    }),
                );
                terminal.write_line(&format!(
                    "release-lease requested: released {released} lease(s)"
                ))?;
                redraw_dashboard = true;
            }
            Some(AppHotkeyAction::ParkEscalate) => {
                let active = workers.iter().filter(|row| row.lease_held).count();
                let task = store.upsert_task(crate::backlog_store::NewTask {
                    kind: TaskKind::Maintenance,
                    title: format!("Escalation requested for {active} active worker(s)"),
                    details: "Operator requested park/escalate from TUI hotkey".to_string(),
                    scope_key: "runtime".to_string(),
                    rationale:
                        "Operator requested immediate attention on active worker saturation."
                            .to_string(),
                    priority: Priority::P0,
                    source: "tui_hotkey".to_string(),
                    related_pr: None,
                    related_branch: None,
                })?;
                terminal.write_line(&format!(
                    "park/escalate requested: created P0 escalation task {}",
                    short_task_id(&task.task_id)
                ))?;
                append_run_log(
                    "warn",
                    "hotkey.park_escalate",
                    json!({
                        "worker_id": WORKER_POOL_ID,
                        "active_workers": active,
                        "task_id": task.task_id
                    }),
                );
                redraw_dashboard = true;
            }
            Some(AppHotkeyAction::ViewReport) => *report_visible = true,
            Some(AppHotkeyAction::RegenerateReport) => {
                let _ = refresh_quality_report(runtime, cfg, scope, true)?;
                *report_visible = true;
            }
            Some(AppHotkeyAction::Back) => {
                *report_visible = false;
                redraw_dashboard = true;
            }
            None => {}
        }
    }
    if *report_visible {
        let report_path = quality_report_path(cfg, scope);
        let report = if runtime.file_system.exists(&report_path) {
            runtime.file_system.read_to_string(&report_path)?
        } else {
            "report not found".to_string()
        };
        terminal.draw_report(&report_path.display().to_string(), &report)?;
    } else if redraw_dashboard {
        let snapshot = dashboard_snapshot(store)?;
        render(
            terminal,
            workers,
            &snapshot,
            cfg.scheduler.heartbeat_interval_seconds,
            cfg.scheduler.lease_timeout_seconds,
        )?;
    }
    Ok(false)
}

fn now_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn append_worker_command(worker: &mut WorkerRow, command: &str) {
    worker
        .command_details
        .push((now_hhmmss(), command.to_string()));
    if worker.command_details.len() > WORKER_COMMAND_HISTORY_LIMIT {
        let overflow = worker.command_details.len() - WORKER_COMMAND_HISTORY_LIMIT;
        worker.command_details.drain(0..overflow);
    }
}

fn set_worker_idle(worker: &mut WorkerRow, tool_line: &str) {
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

fn refresh_worker_heartbeats(workers: &mut [WorkerRow], last_activity_pulse: &[Instant]) {
    let now = Instant::now();
    for (idx, worker) in workers.iter_mut().enumerate() {
        if let Some(last_pulse) = last_activity_pulse.get(idx) {
            let age = now.duration_since(*last_pulse).as_secs();
            worker.last_heartbeat_secs = age;
            worker.session_age_secs = age;
        }
    }
}

fn now_hhmmss() -> String {
    let timestamp = now_unix_millis().rem_euclid(86_400_000);
    let secs = (timestamp / 1000) as u64;
    let in_day = secs % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        in_day / 3600,
        (in_day % 3600) / 60,
        in_day % 60
    )
}

fn append_worker_tool_commands(
    workers: &mut [WorkerRow],
    last_worker_command_line: &mut usize,
    max_events: usize,
) -> bool {
    let events = recent_worker_tool_commands(*last_worker_command_line, max_events);
    let mut updated = false;
    for (line, worker_id, command) in events {
        let mut matched = false;
        for worker in workers.iter_mut() {
            if worker.worker_id == worker_id {
                append_worker_command(worker, &command);
                matched = true;
                updated = true;
                break;
            }
        }
        *last_worker_command_line = line + 1;
        if !matched {
            continue;
        }
    }
    updated
}

fn append_worker_state_events(
    workers: &mut [WorkerRow],
    last_worker_state_line: &mut usize,
    max_events: usize,
) -> bool {
    let events = recent_worker_state_events(*last_worker_state_line, max_events);
    let mut updated = false;
    for (line, worker_id, state, task_id, details) in events {
        for worker in workers.iter_mut() {
            if worker.worker_id != worker_id {
                continue;
            }
            if worker.task_id.as_deref() != Some(task_id.as_str()) {
                continue;
            }
            if !is_non_regressive_state_transition(&worker.state, &state) {
                worker.last_state_line = line + 1;
                continue;
            }
            let details = details.trim();
            let state_label = format_state_label(&state);
            let tool_line = if details.is_empty() {
                state_label
            } else {
                format!("{state_label} ({details})")
            };
            if worker.state != state || worker.tool_line != tool_line {
                worker.state = state.clone();
                worker.breadcrumb = format!("state>{state}");
                worker.tool_line = tool_line;
                if details.is_empty() {
                    append_worker_command(worker, &format!("state {state}"));
                } else {
                    append_worker_command(worker, &format!("state {state}: {details}"));
                }
                worker.last_state_line = line + 1;
                updated = true;
            } else {
                worker.last_state_line = line + 1;
            }
            break;
        }
        *last_worker_state_line = line + 1;
    }
    updated
}

fn is_non_regressive_state_transition(current_state: &str, next_state: &str) -> bool {
    let current = normalize_worker_state_for_transition(current_state);
    let next = normalize_worker_state_for_transition(next_state);

    match current {
        "complete" | "failed" | "unresolved" | "parked" | "idle" => return false,
        _ => {}
    }
    if next == "unknown" {
        return false;
    }
    let current_rank = worker_state_rank(current);
    let next_rank = worker_state_rank(next);
    next_rank >= current_rank
}

fn worker_state_rank(state: &str) -> i32 {
    match state {
        "understand" => 0,
        "planning" => 1,
        "doing" => 2,
        "gitting" => 3,
        "reviewing" => 4,
        "merging" => 5,
        "complete" => 6,
        "failed" => 7,
        "unresolved" => 8,
        "parked" => 9,
        "idle" => 10,
        _ => 10,
    }
}

fn normalize_worker_state_for_transition(state: &str) -> &str {
    let normalized_state = state.trim().to_ascii_lowercase();
    let normalized_state = normalized_state
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .rfind(|part| !part.is_empty())
        .unwrap_or(normalized_state.as_str());

    match normalized_state {
        "init" | "boot" | "backlog_sync" | "working" | "seeding" => "understand",
        "claimed" | "starting" | "worktree_preparing" | "worktree_ready" => "understand",
        "commit" | "gitting_remediation" | "pr_creating" => "gitting",
        "merge_lock_waiting"
        | "merge_from_main"
        | "merge_lock_held"
        | "merge_polling"
        | "handoff"
        | "merge_remediation"
        | "post_merge_validation"
        | "teardown" => "merging",
        "understand" => "understand",
        "planning" => "planning",
        "doing" => "doing",
        "gitting" => "gitting",
        "reviewing" => "reviewing",
        "merging" => "merging",
        "complete" => "complete",
        "failed" => "failed",
        "unresolved" => "unresolved",
        "idle" => "idle",
        "parked" => "parked",
        _ => "unknown",
    }
}

fn is_copy_shortcut_key(key: char) -> bool {
    key.eq_ignore_ascii_case(&COPY_SHORTCUT_KEY)
}

fn hotkey_action(key: char, operator_hotkeys: bool) -> Option<AppHotkeyAction> {
    action_for_key_with_mode(key, operator_hotkeys)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DashboardSnapshot {
    stats: QueueStats,
    backlog: BacklogView,
}

fn dashboard_snapshot(store: &BacklogStore) -> Result<DashboardSnapshot, GardenerError> {
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
            crate::backlog_store::TaskStatus::Ready => stats.ready += 1,
            crate::backlog_store::TaskStatus::Leased
            | crate::backlog_store::TaskStatus::InProgress => {
                stats.active += 1;
                backlog.in_progress.push(format!(
                    "INP {} {} {}",
                    task.priority.as_str(),
                    short_task_id(&task.task_id),
                    task.title
                ));
            }
            crate::backlog_store::TaskStatus::MergePending => {
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
            crate::backlog_store::TaskStatus::Failed => stats.failed += 1,
            crate::backlog_store::TaskStatus::Unresolved => stats.unresolved += 1,
            crate::backlog_store::TaskStatus::Complete => {}
        }
        if matches!(task.status, crate::backlog_store::TaskStatus::Ready) {
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

fn render(
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

fn short_task_id(task_id: &str) -> &str {
    task_id.get(0..6).unwrap_or(task_id)
}

fn worker_failure_prompt(worker_id: &str, task_id: &str, reason: &str) -> String {
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

fn quality_report_path(cfg: &AppConfig, scope: &RuntimeScope) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(&cfg.quality_report.path);
    if path.is_absolute() {
        path
    } else {
        scope.working_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        hotkey_action, is_non_regressive_state_transition, run_worker_pool_fsm, wait_for_quit,
        INTERRUPT_SENTINEL_KEY,
    };
    use crate::backlog_store::{BacklogStore, NewTask};
    use crate::config::AppConfig;
    use crate::hotkeys::{action_for_key, HotkeyAction, DASHBOARD_BINDINGS, REPORT_BINDINGS};
    use crate::logging::{clear_run_logger, init_run_logger};
    use crate::priority::Priority;
    use crate::runtime::{
        FakeClock, FakeProcessRunner, FakeTerminal, ProductionFileSystem, ProductionRuntime,
    };
    use crate::task_identity::TaskKind;
    use crate::types::RuntimeScope;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
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
    fn report_hotkey_actions_cover_report_bindings() {
        for binding in REPORT_BINDINGS {
            let action = hotkey_action(binding.key, false);
            assert!(action.is_some());
        }
    }

    #[test]
    fn hotkey_actions_match_default_and_operator_contracts() {
        assert_eq!(hotkey_action('q', false), Some(HotkeyAction::Quit)); // hotkey:q
        assert_eq!(hotkey_action('j', false), Some(HotkeyAction::ScrollDown)); // hotkey:j
        assert_eq!(hotkey_action('k', false), Some(HotkeyAction::ScrollUp)); // hotkey:k
        assert_eq!(hotkey_action('c', false), None); // hotkey:c removed
        assert_eq!(hotkey_action('v', false), Some(HotkeyAction::ViewReport)); // hotkey:v
        assert_eq!(
            hotkey_action('g', false),
            Some(HotkeyAction::RegenerateReport)
        ); // hotkey:g
        assert_eq!(hotkey_action('b', false), Some(HotkeyAction::Back)); // hotkey:b
        assert_eq!(hotkey_action('r', false), None);
        assert_eq!(hotkey_action('l', false), None);
        assert_eq!(hotkey_action('p', false), None);
        assert_eq!(hotkey_action('c', true), None); // hotkey:c removed

        assert_eq!(hotkey_action('r', true), Some(HotkeyAction::Retry)); // hotkey:r
        assert_eq!(hotkey_action('l', true), Some(HotkeyAction::ReleaseLease)); // hotkey:l
        assert_eq!(hotkey_action('p', true), Some(HotkeyAction::ParkEscalate)); // hotkey:p
        assert_eq!(hotkey_action('x', true), None);
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
            include_str!("../tests/fixtures/triage/expected-profiles/phase03-profile.toml"),
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
            include_str!("../tests/fixtures/triage/expected-profiles/phase03-profile.toml"),
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
    fn run_worker_pool_does_not_size_workers_from_target() {
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
}
