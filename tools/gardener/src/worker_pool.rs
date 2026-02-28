use crate::backlog_store::BacklogStore;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::hotkeys::{
    action_for_key_with_mode, operator_hotkeys_enabled, HotkeyAction as AppHotkeyAction,
};
use crate::logging::{
    append_run_log, current_log_line_count, recent_worker_log_lines, recent_worker_tool_commands,
    structured_fallback_line,
};
use crate::priority::Priority;
use crate::runtime::Terminal;
use crate::runtime::{clear_interrupt, request_interrupt, ProductionRuntime};
use crate::startup::refresh_quality_report;
use crate::task_identity::TaskKind;
use crate::tui::{
    reset_workers_scroll, scroll_workers_down, scroll_workers_up,
    BacklogView, QueueStats, WorkerRow,
};
use crate::types::RuntimeScope;
use crate::worker::execute_task;
use serde_json::json;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const WORKER_POOL_ID: &str = "worker_pool";
const WORKER_COMMAND_HISTORY_LIMIT: usize = 32;

type WorkerResultMessage = (
    usize,
    String,
    Result<crate::worker::WorkerRunSummary, GardenerError>,
);

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
    let configured_parallelism = cfg.orchestrator.parallelism.max(1) as usize;
    let parallelism = configured_parallelism.min(target.max(1));
    let mut workers = (0..parallelism)
        .map(|idx| WorkerRow {
            worker_id: format!("worker-{}", idx + 1),
            state: "idle".to_string(),
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
            append_worker_command(&mut worker, "waiting for claim");
            worker
        })
        .collect::<Vec<_>>();
    let mut last_worker_command_line = current_log_line_count();
    let command_poll_chunk = 32;
    let mut completed = 0usize;
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
        let mut claimed = Vec::new();
        let mut claimed_any = false;
        let available_slots = parallelism.min(target.saturating_sub(completed));
        for idx in 0..available_slots {
            let worker_id = workers[idx].worker_id.clone();
            let claimed_task = store.claim_next(&worker_id, cfg.scheduler.lease_timeout_seconds as i64)?;
            let Some(task) = claimed_task else {
                workers[idx].state = "idle".to_string();
                workers[idx].task_title = "idle".to_string();
                workers[idx].lease_held = false;
                if workers[idx].tool_line != "waiting for claim" {
                    append_worker_command(&mut workers[idx], "waiting for claim");
                }
                workers[idx].tool_line = "waiting for claim".to_string();
                continue;
            };
            claimed_any = true;
            append_run_log(
                "info",
                "worker.task.claimed",
                json!({
                    "worker_id": worker_id,
                    "task_id": task.task_id,
                    "title": task.title
                }),
            );
            let _ = store.mark_in_progress(&task.task_id, &worker_id)?;

            workers[idx].state = "doing".to_string();
            workers[idx].task_title = task.title.clone();
            workers[idx].tool_line = "claimed".to_string();
            workers[idx].breadcrumb = "claim>doing".to_string();
            workers[idx].lease_held = true;
            append_worker_command(&mut workers[idx], "claimed");
            render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;
            claimed.push((idx, task));
        }

        if !claimed_any {
            break;
        }

        let mut active = claimed.len();
        let mut shutdown_error: Option<(String, String, String)> = None;
        let mut quit_requested = false;
        let (tx, rx): (mpsc::Sender<WorkerResultMessage>, mpsc::Receiver<WorkerResultMessage>) =
            mpsc::channel();
        let runtime_scope = scope.clone();
        let mut last_dashboard_refresh = Instant::now();

        std::thread::scope(|scope_guard| -> Result<(), GardenerError> {
            for (idx, task) in claimed {
                let tx = tx.clone();
                let worker_id = workers[idx].worker_id.clone();
                let task_id = task.task_id.clone();
                let task_summary = task_override.unwrap_or(task.title.as_str()).to_string();
                let attempt_count = task.attempt_count;
                let cfg = cfg.clone();
                let process_runner = runtime.process_runner.clone();
                let worker_scope = runtime_scope.clone();
                scope_guard.spawn(move || {
                    let result = execute_task(
                        &cfg,
                        process_runner.as_ref(),
                        &worker_scope,
                        &worker_id,
                        &task_id,
                        &task_summary,
                        attempt_count,
                    );
                    let _ = tx.send((idx, task_id, result));
                });
            }
            drop(tx);

            while active > 0 {
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
                    quit_requested = true;
                }

                match rx.recv_timeout(Duration::from_millis(25)) {
                    Ok((idx, task_id, turn_result)) => {
                        active = active.saturating_sub(1);
                        let worker_id = workers[idx].worker_id.clone();
                        if shutdown_error.is_none() {
                            let summary = match turn_result {
                                Ok(summary) => summary,
                                Err(GardenerError::Process(message))
                                    if message.contains("user interrupt requested") =>
                                {
                                    quit_requested = true;
                                    continue;
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
                                    shutdown_error = Some((worker_id, task_id, msg));
                                    request_interrupt();
                                    continue;
                                }
                            };
                            for event in summary.logs {
                                workers[idx].state = event.state.as_str().to_string();
                                let prompt = format!("prompt {}", event.prompt_version);
                                workers[idx].tool_line = prompt.clone();
                                append_worker_command(&mut workers[idx], &prompt);
                                workers[idx].breadcrumb =
                                    format!("state>{}", event.state.as_str());
                                last_activity_pulse[idx] = Instant::now();
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
                                let completed_message = format!("completed {}", task_id);
                                workers[idx].tool_line = completed_message.clone();
                                append_worker_command(&mut workers[idx], &completed_message);
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
                                let failed_message = if let Some(reason) = summary.failure_reason.clone() {
                                    if reason.is_empty() {
                                        format!("failed {}", task_id)
                                    } else {
                                        let truncated = reason
                                            .chars()
                                            .take(150)
                                            .collect::<String>();
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
                                    && summary.failure_reason.is_none()
                                {
                                    let unresolved = store.mark_unresolved(&task_id, &worker_id)?;
                                    append_run_log(
                                        "warn",
                                        "worker.task.unresolved",
                                        json!({
                                            "worker_id": worker_id,
                                            "task_id": task_id,
                                            "marked_unresolved": unresolved,
                                        }),
                                    );
                                    let unresolved_message = if unresolved {
                                        format!("unresolved {}", task_id)
                                    } else {
                                        failed_message.clone()
                                    };
                                    workers[idx].state = "unresolved".to_string();
                                    workers[idx].tool_line = unresolved_message.clone();
                                    workers[idx].breadcrumb = "unresolved".to_string();
                                    append_worker_command(&mut workers[idx], &unresolved_message);
                                } else if let Some(reason) = summary.failure_reason {
                                    shutdown_error = Some((worker_id.clone(), task_id.clone(), reason));
                                } else {
                                    let _ = store.release_lease(&task_id, &worker_id)?;
                                }
                            }
                        } else {
                            request_interrupt();
                        }
                        render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let updated_commands = append_worker_tool_commands(
                            &mut workers,
                            &mut last_worker_command_line,
                            command_poll_chunk,
                        );
                        if updated_commands || last_dashboard_refresh.elapsed() >= Duration::from_secs(1) {
                            render(
                                terminal,
                                &workers,
                                &dashboard_snapshot(store)?,
                                hb,
                                lt,
                            )?;
                            last_dashboard_refresh = Instant::now();
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        active = 0;
                    }
                }
            }
            Ok(())
        })?;

        if let Some((worker_id, task_id, reason)) = shutdown_error {
            if terminal.stdin_is_tty() {
                terminal.draw_shutdown_screen("Error", &worker_failure_prompt(&worker_id, &task_id, &reason))?;
            } else {
                terminal.write_line(&format!("Error: worker {worker_id} task {task_id}: {reason}"))?;
            }
            wait_for_quit(terminal)?;
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
    wait_for_quit(terminal)?;
    Ok(completed)
}

fn wait_for_quit(terminal: &dyn Terminal) -> Result<(), GardenerError> {
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
                let active = workers.iter().filter(|row| row.state == "doing").count();
                let task = store.upsert_task(crate::backlog_store::NewTask {
                    kind: TaskKind::Maintenance,
                    title: format!("Escalation requested for {active} active worker(s)"),
                    details: "Operator requested park/escalate from TUI hotkey".to_string(),
                    scope_key: "runtime".to_string(),
                    rationale: "Operator requested immediate attention on active worker saturation."
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

fn hotkey_action(key: char, operator_hotkeys: bool) -> Option<AppHotkeyAction> {
    action_for_key_with_mode(key, operator_hotkeys)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DashboardSnapshot {
    stats: QueueStats,
    backlog: BacklogView,
}

fn dashboard_snapshot(store: &BacklogStore) -> Result<DashboardSnapshot, GardenerError> {
    append_run_log(
        "debug",
        "worker_pool.dashboard_snapshot.started",
        json!({
            "worker_id": WORKER_POOL_ID
        }),
    );
    let tasks = store.list_tasks()?;
    let mut stats = QueueStats {
        ready: 0,
        active: 0,
        failed: 0,
        unresolved: 0,
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
    append_run_log(
        "debug",
        "dashboard.snapshot",
        json!({
            "worker_id": WORKER_POOL_ID,
            "workers": workers.len(),
            "ready": snapshot.stats.ready,
            "active": snapshot.stats.active,
            "failed": snapshot.stats.failed,
            "unresolved": snapshot.stats.unresolved,
            "backlog_in_progress": snapshot.backlog.in_progress.len(),
            "backlog_queued": snapshot.backlog.queued.len(),
            "tty": terminal.stdin_is_tty()
        }),
    );
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
    use super::{hotkey_action, run_worker_pool_fsm};
    use crate::errors::GardenerError;
    use crate::backlog_store::{BacklogStore, NewTask};
    use crate::config::AppConfig;
    use crate::hotkeys::{action_for_key, HotkeyAction, DASHBOARD_BINDINGS, REPORT_BINDINGS};
    use crate::priority::Priority;
    use crate::runtime::{
        FakeClock, FakeProcessRunner, FakeTerminal, ProductionFileSystem, ProductionRuntime, Terminal,
    };
    use crate::task_identity::TaskKind;
    use crate::types::RuntimeScope;
    use std::path::PathBuf;
    use std::sync::Arc;
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
        assert!(!writes.iter().any(|line| line.contains("worker-2")));
    }
}
