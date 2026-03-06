use super::dashboard::{dashboard_snapshot, quality_report_path, render, short_task_id};
use super::util::now_unix_millis;
use super::{COPY_SHORTCUT_KEY, WORKER_POOL_ID};
use crate::backlog_store::{BacklogStore, NewTask};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::hotkeys::{action_for_key_with_mode, HotkeyAction as AppHotkeyAction};
use crate::logging::append_run_log;
use crate::priority::Priority;
use crate::runtime::Terminal;
use crate::runtime::{
    request_interrupt, ProductionRuntime, ARROW_DOWN_SENTINEL, ARROW_UP_SENTINEL,
    INTERRUPT_SENTINEL_KEY,
};
use crate::startup::refresh_quality_report;
use crate::task_identity::TaskKind;
use crate::tui::{
    reset_report_scroll, scroll_report_down, scroll_report_up, scroll_workers_down,
    scroll_workers_up, WorkerRow,
};
use crate::types::RuntimeScope;
use serde_json::json;

pub(super) struct HotkeyState<'a> {
    pub(super) runtime: &'a ProductionRuntime,
    pub(super) scope: &'a RuntimeScope,
    pub(super) cfg: &'a AppConfig,
    pub(super) store: &'a BacklogStore,
    pub(super) workers: &'a mut [WorkerRow],
    pub(super) operator_hotkeys: bool,
    pub(super) terminal: &'a dyn Terminal,
    pub(super) report_visible: &'a mut bool,
    pub(super) report_content: &'a mut Option<String>,
}

pub(super) fn wait_for_quit(
    terminal: &dyn Terminal,
    copy_target: Option<&str>,
) -> Result<(), GardenerError> {
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
    loop {
        match terminal.poll_key(100)? {
            Some(INTERRUPT_SENTINEL_KEY) => {
                copy_to_clipboard_if_present(terminal, copy_target);
                return Ok(());
            }
            Some(key) if is_copy_shortcut_key(key) && copy_target.is_some() => {
                copy_to_clipboard_if_present(terminal, copy_target);
                return Ok(());
            }
            Some(_) => return Ok(()),
            None => {
                if !crate::runtime::KEY_LISTENER_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(());
                }
            }
        }
    }
}

pub(super) fn handle_hotkeys(state: &mut HotkeyState<'_>) -> Result<bool, GardenerError> {
    let runtime = state.runtime;
    let scope = state.scope;
    let cfg = state.cfg;
    let store = state.store;
    let workers = &mut *state.workers;
    let operator_hotkeys = state.operator_hotkeys;
    let terminal = state.terminal;
    let report_visible = &mut *state.report_visible;
    let report_content = &mut *state.report_content;

    if !terminal.stdin_is_tty() {
        return Ok(false);
    }
    let mut redraw_dashboard = false;
    let mut redraw_report = false;
    if let Some(key) = terminal.poll_key(10)? {
        if key == '\0' {
            if *report_visible {
                redraw_report = true;
            } else {
                redraw_dashboard = true;
            }
        }
        if key == INTERRUPT_SENTINEL_KEY {
            append_run_log(
                "warn",
                "hotkey.quit",
                json!({ "worker_id": WORKER_POOL_ID }),
            );
            request_interrupt();
            return Ok(true);
        }

        if *report_visible {
            match key {
                c if c == 'j' || c == ARROW_DOWN_SENTINEL => {
                    let (_, h) = terminal.draw_dimensions();
                    let viewport = h.saturating_sub(8) as usize;
                    if scroll_report_down(viewport) {
                        redraw_report = true;
                    }
                }
                c if c == 'k' || c == ARROW_UP_SENTINEL => {
                    if scroll_report_up() {
                        redraw_report = true;
                    }
                }
                'b' => {
                    *report_visible = false;
                    *report_content = None;
                    reset_report_scroll();
                    redraw_dashboard = true;
                }
                'g' => {
                    let _ = refresh_quality_report(runtime, cfg, scope, true)?;
                    *report_content = Some(load_report_content(runtime, cfg, scope));
                    reset_report_scroll();
                    redraw_report = true;
                }
                'q' => {
                    append_run_log(
                        "warn",
                        "hotkey.quit",
                        json!({ "worker_id": WORKER_POOL_ID }),
                    );
                    request_interrupt();
                    return Ok(true);
                }
                _ => {}
            }
        } else {
            match hotkey_action(key, operator_hotkeys) {
                Some(AppHotkeyAction::Quit) => {
                    append_run_log(
                        "warn",
                        "hotkey.quit",
                        json!({ "worker_id": WORKER_POOL_ID }),
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
                        json!({ "worker_id": WORKER_POOL_ID, "released": released }),
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
                        json!({ "worker_id": WORKER_POOL_ID, "released": released }),
                    );
                    terminal.write_line(&format!(
                        "release-lease requested: released {released} lease(s)"
                    ))?;
                    redraw_dashboard = true;
                }
                Some(AppHotkeyAction::ParkEscalate) => {
                    let active = workers.iter().filter(|row| row.lease_held).count();
                    let task = store.upsert_task(NewTask {
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
                Some(AppHotkeyAction::ViewReport) => {
                    *report_visible = true;
                    *report_content = Some(load_report_content(runtime, cfg, scope));
                    reset_report_scroll();
                    redraw_report = true;
                }
                Some(AppHotkeyAction::RegenerateReport) => {
                    let _ = refresh_quality_report(runtime, cfg, scope, true)?;
                    *report_visible = true;
                    *report_content = Some(load_report_content(runtime, cfg, scope));
                    reset_report_scroll();
                    redraw_report = true;
                }
                Some(AppHotkeyAction::Back) => {}
                None => {}
            }
        }
    }
    if *report_visible && redraw_report {
        let report_path = quality_report_path(cfg, scope);
        let report = report_content.as_deref().unwrap_or("report not found");
        terminal.draw_report(&report_path.display().to_string(), report)?;
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

fn load_report_content(
    runtime: &ProductionRuntime,
    cfg: &AppConfig,
    scope: &RuntimeScope,
) -> String {
    let report_path = quality_report_path(cfg, scope);
    if runtime.file_system.exists(&report_path) {
        runtime
            .file_system
            .read_to_string(&report_path)
            .unwrap_or_else(|_| "report not found".to_string())
    } else {
        "report not found".to_string()
    }
}

fn hotkey_action(key: char, operator_hotkeys: bool) -> Option<AppHotkeyAction> {
    action_for_key_with_mode(key, operator_hotkeys)
}

fn is_copy_shortcut_key(key: char) -> bool {
    key.eq_ignore_ascii_case(&COPY_SHORTCUT_KEY)
}

fn copy_to_clipboard_if_present(terminal: &dyn Terminal, copy_target: Option<&str>) {
    let Some(target) = copy_target else {
        return;
    };
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
