use gardener::backlog_store::BacklogStore;
use gardener::config::AppConfig;
use gardener::hotkeys::{action_for_key_with_mode, operator_hotkeys_enabled, HotkeyAction};
use gardener::priority::Priority;
use gardener::runtime::{
    FakeClock, FakeProcessRunner, FakeTerminal, ProductionFileSystem, ProductionRuntime,
};
use gardener::types::RuntimeScope;
use gardener::worker_pool::run_worker_pool_fsm;
use std::env;
use std::ffi::OsString;
use std::sync::Arc;
use tempfile::TempDir;

fn test_scope(dir: &TempDir) -> RuntimeScope {
    RuntimeScope {
        process_cwd: dir.path().to_path_buf(),
        working_dir: dir.path().to_path_buf(),
        repo_root: Some(dir.path().to_path_buf()),
    }
}

fn test_runtime(terminal: FakeTerminal) -> ProductionRuntime {
    ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(ProductionFileSystem),
        process_runner: Arc::new(FakeProcessRunner::default()),
        terminal: Arc::new(terminal),
    }
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let previous = env::var_os(key);
        match value {
            Some(value) => env::set_var(key, value),
            None => env::remove_var(key),
        }
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.clone() {
            Some(value) => env::set_var(self.key, value),
            None => env::remove_var(self.key),
        }
    }
}

#[test]
fn escalate_creates_task_even_with_zero_active_workers() {
    // BUG B4: escalation uses active worker count with no guard, so 0 is accepted.
    let dir = TempDir::new().expect("tempdir");
    let scope = test_scope(&dir);
    let store =
        BacklogStore::open(dir.path().join(".cache/gardener/backlog.sqlite")).expect("open store");

    let mut cfg = AppConfig::default();
    cfg.execution.test_mode = true;
    cfg.orchestrator.parallelism = 1;

    let terminal = FakeTerminal::new(true);
    terminal.enqueue_keys(['p']);
    let runtime = test_runtime(terminal.clone());
    let _guard = EnvVarGuard::set("GARDENER_OPERATOR_HOTKEYS", Some("1"));

    let completed = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 1, None)
        .expect("run worker pool");
    assert_eq!(completed, 1);

    let tasks = store.list_tasks().expect("list tasks");
    assert!(
        tasks.iter().any(|task| task
            .title
            .contains("Escalation requested for 0 active worker(s)")
            && task.priority == Priority::P0),
        "expected operator-initiated P0 task with active count in the title"
    );
}

#[test]
fn quit_after_zero_shows_shutdown_screen() {
    let dir = TempDir::new().expect("tempdir");
    let scope = test_scope(&dir);
    let store =
        BacklogStore::open(dir.path().join(".cache/gardener/backlog.sqlite")).expect("open store");

    let mut cfg = AppConfig::default();
    cfg.execution.test_mode = true;
    cfg.orchestrator.parallelism = 1;

    let terminal = FakeTerminal::new(true);
    let runtime = test_runtime(terminal.clone());

    let completed = run_worker_pool_fsm(&runtime, &scope, &cfg, &store, &terminal, 0, None)
        .expect("run worker pool");
    assert_eq!(completed, 0);

    let shutdown_screens = terminal.shutdown_screens();
    let shutdown = shutdown_screens.first().expect("shutdown screen was shown");
    assert_eq!(shutdown.0, "All Tasks Complete");
    assert_eq!(shutdown.1, "Completed 0 of 0 task(s).");
}

#[test]
fn operator_hotkeys_env_var_truthy_variants() {
    let truthy = ["1", "true", "TRUE", "yes", "Yes", " 1 "];
    for value in truthy {
        let _guard = EnvVarGuard::set("GARDENER_OPERATOR_HOTKEYS", Some(value));
        assert!(operator_hotkeys_enabled());
    }

    let falsy = ["", "0", "false", "no", "maybe"];
    for value in falsy {
        let _guard = EnvVarGuard::set("GARDENER_OPERATOR_HOTKEYS", Some(value));
        assert!(!operator_hotkeys_enabled());
    }

    let removed = EnvVarGuard::set("GARDENER_OPERATOR_HOTKEYS", None);
    assert!(!operator_hotkeys_enabled());
    drop(removed);
    assert!(!operator_hotkeys_enabled());
}

#[test]
fn operator_hotkey_mapping_unchanged() {
    let action = action_for_key_with_mode('p', false);
    assert_eq!(action, None);

    let action = action_for_key_with_mode('p', true);
    assert_eq!(action, Some(HotkeyAction::ParkEscalate));
}

#[test]
fn unknown_hotkey_does_not_corrupt_known_actions() {
    assert_eq!(action_for_key_with_mode('x', true), None);
    assert_eq!(
        action_for_key_with_mode('r', true),
        Some(HotkeyAction::Retry)
    );
    assert_eq!(action_for_key_with_mode('x', true), None);
    assert_eq!(
        action_for_key_with_mode('l', true),
        Some(HotkeyAction::ReleaseLease)
    );
}
