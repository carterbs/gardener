use crate::backlog_store::BacklogStore;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::hotkeys::{
    action_for_key_with_mode, operator_hotkeys_enabled, HotkeyAction as AppHotkeyAction,
};
use crate::logging::structured_fallback_line;
use crate::priority::Priority;
use crate::runtime::Terminal;
use crate::runtime::{clear_interrupt, request_interrupt, ProductionRuntime};
use crate::startup::refresh_quality_report;
use crate::task_identity::TaskKind;
use crate::tui::{BacklogView, QueueStats, WorkerRow};
use crate::types::RuntimeScope;
use crate::worker::execute_task;

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
    let operator_hotkeys = operator_hotkeys_enabled();
    let mut report_visible = false;
    let hb = cfg.scheduler.heartbeat_interval_seconds;
    let lt = cfg.scheduler.lease_timeout_seconds;
    let parallelism = (cfg.orchestrator.parallelism.max(1) as usize).min(target.max(1));
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
        })
        .collect::<Vec<_>>();
    let mut completed = 0usize;
    render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;

    while completed < target {
        if handle_hotkeys(
            runtime,
            scope,
            cfg,
            store,
            &workers,
            operator_hotkeys,
            terminal,
            &mut report_visible,
        )? {
            return Ok(completed);
        }
        if report_visible {
            continue;
        }
        let mut claimed_any = false;
        for idx in 0..workers.len() {
            if handle_hotkeys(
                runtime,
                scope,
                cfg,
                store,
                &workers,
                operator_hotkeys,
                terminal,
                &mut report_visible,
            )? {
                return Ok(completed);
            }
            if report_visible {
                break;
            }
            if completed >= target {
                break;
            }

            let worker_id = workers[idx].worker_id.clone();
            let claimed =
                store.claim_next(&worker_id, cfg.scheduler.lease_timeout_seconds as i64)?;
            let Some(task) = claimed else {
                workers[idx].state = "idle".to_string();
                workers[idx].task_title = "idle".to_string();
                workers[idx].tool_line = "waiting for claim".to_string();
                workers[idx].lease_held = false;
                continue;
            };
            claimed_any = true;
            let _ = store.mark_in_progress(&task.task_id, &worker_id)?;

            workers[idx].state = "doing".to_string();
            workers[idx].task_title = task.title.clone();
            workers[idx].tool_line = "claimed".to_string();
            workers[idx].breadcrumb = "claim>doing".to_string();
            workers[idx].lease_held = true;
            render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;

            let mut quit_requested = false;
            let mut turn_result: Option<Result<crate::worker::WorkerRunSummary, GardenerError>> =
                None;
            std::thread::scope(|scope_guard| {
                let (tx, rx) = std::sync::mpsc::channel();
                let worker_id = worker_id.clone();
                let task_id = task.task_id.clone();
                let task_summary = task_override
                    .unwrap_or(task.title.as_str())
                    .to_string();
                scope_guard.spawn(move || {
                    let result = execute_task(
                        cfg,
                        runtime.process_runner.as_ref(),
                        scope,
                        &worker_id,
                        &task_id,
                        &task_summary,
                    );
                    let _ = tx.send(result);
                });

                loop {
                    match rx.recv_timeout(std::time::Duration::from_millis(25)) {
                        Ok(result) => {
                            turn_result = Some(result);
                            break;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if handle_hotkeys(
                                runtime,
                                scope,
                                cfg,
                                store,
                                &workers,
                                operator_hotkeys,
                                terminal,
                                &mut report_visible,
                            )
                            .unwrap_or(false)
                            {
                                request_interrupt();
                                quit_requested = true;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            turn_result = Some(Err(GardenerError::Process(
                                "worker turn channel disconnected".to_string(),
                            )));
                            break;
                        }
                    }
                }
            });
            let summary = match turn_result.expect("worker result set by scoped worker thread") {
                Ok(summary) => summary,
                Err(GardenerError::Process(message))
                    if message.contains("user interrupt requested") =>
                {
                    return Ok(completed);
                }
                Err(err) => return Err(err),
            };
            if quit_requested {
                return Ok(completed);
            }
            for event in summary.logs {
                workers[idx].state = event.state.as_str().to_string();
                workers[idx].tool_line = format!("prompt {}", event.prompt_version);
                workers[idx].breadcrumb = format!("state>{}", event.state.as_str());
                render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;
            }

            if summary.final_state == crate::types::WorkerState::Complete {
                let _ = store.mark_complete(&task.task_id, &worker_id)?;
                completed = completed.saturating_add(1);
                workers[idx].state = "complete".to_string();
                workers[idx].tool_line = format!("completed {}", task.task_id);
                workers[idx].lease_held = false;
            } else {
                workers[idx].state = "failed".to_string();
                workers[idx].tool_line = format!("failed {}", task.task_id);
            }
            render(terminal, &workers, &dashboard_snapshot(store)?, hb, lt)?;
        }

        if !claimed_any {
            break;
        }
    }
    Ok(completed)
}

fn handle_hotkeys(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    store: &BacklogStore,
    workers: &[WorkerRow],
    operator_hotkeys: bool,
    terminal: &dyn Terminal,
    report_visible: &mut bool,
) -> Result<bool, GardenerError> {
    if !terminal.stdin_is_tty() {
        return Ok(false);
    }
    let mut redraw_dashboard = false;
    if let Some(key) = terminal.poll_key(10)? {
        match hotkey_action(key, operator_hotkeys) {
            Some(AppHotkeyAction::Quit) => {
                request_interrupt();
                return Ok(true);
            }
            Some(AppHotkeyAction::Retry) => {
                let released = store.recover_stale_leases(now_unix_millis())?;
                terminal.write_line(&format!(
                    "retry requested: released {released} stale lease(s)"
                ))?;
                redraw_dashboard = true;
            }
            Some(AppHotkeyAction::ReleaseLease) => {
                let release_now = now_unix_millis()
                    .saturating_add((cfg.scheduler.lease_timeout_seconds as i64 + 1) * 1000);
                let released = store.recover_stale_leases(release_now)?;
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
                    priority: Priority::P0,
                    source: "tui_hotkey".to_string(),
                    related_pr: None,
                    related_branch: None,
                })?;
                terminal.write_line(&format!(
                    "park/escalate requested: created P0 escalation task {}",
                    short_task_id(&task.task_id)
                ))?;
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
    use crate::backlog_store::{BacklogStore, NewTask};
    use crate::config::AppConfig;
    use crate::hotkeys::{action_for_key, HotkeyAction, DASHBOARD_BINDINGS, REPORT_BINDINGS};
    use crate::priority::Priority;
    use crate::runtime::{
        FakeClock, FakeProcessRunner, FakeTerminal, ProductionFileSystem, ProductionRuntime,
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
        assert_eq!(hotkey_action('v', false), Some(HotkeyAction::ViewReport)); // hotkey:v
        assert_eq!(
            hotkey_action('g', false),
            Some(HotkeyAction::RegenerateReport)
        ); // hotkey:g
        assert_eq!(hotkey_action('b', false), Some(HotkeyAction::Back)); // hotkey:b
        assert_eq!(hotkey_action('r', false), None);
        assert_eq!(hotkey_action('l', false), None);
        assert_eq!(hotkey_action('p', false), None);

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
