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
