use expectrl::{Eof, Expect};
use gardener::backlog_store::{BacklogStore, NewTask, TaskStatus};
use gardener::priority::Priority;
use gardener::task_identity::TaskKind;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

fn upsert_task(store: &BacklogStore, title: &str) {
    let _ = store
        .upsert_task(NewTask {
            kind: TaskKind::Maintenance,
            title: title.to_string(),
            details: "details".to_string(),
            rationale: String::new(),
            scope_key: "scope".to_string(),
            priority: Priority::P1,
            source: "pty-test".to_string(),
            related_pr: None,
            related_branch: None,
        })
        .expect("upsert task");
}

fn write_exec(path: &std::path::Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, contents).expect("write script");
    let mut perms = std::fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod");
}

fn setup_pty_fixture() -> (std::path::PathBuf, TempDir, BacklogStore, Command) {
    let bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_gardener"));
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let profile_path = dir.path().join(".gardener/repo-intelligence.toml");
    let report_path = dir.path().join(".gardener/quality.md");

    std::fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("mkdir profile");
    std::fs::write(
        &profile_path,
        include_str!("fixtures/triage/expected-profiles/phase03-profile.toml"),
    )
    .expect("write profile");
    std::fs::write(&report_path, "OLD_MARKER").expect("write report");

    std::fs::write(
        &config_path,
        format!(
            r#"[scope]
working_dir = "{}"

[validation]
command = "npm run validate"
allow_agent_discovery = true

[agent]
default = "codex"

[execution]
permissions_mode = "permissive_v1"
worker_mode = "normal"
test_mode = true

[orchestrator]
parallelism = 1

[triage]
output_path = "{}"
stale_after_commits = 50
discovery_max_turns = 12

[quality_report]
path = "{}"
stale_after_days = 7
stale_if_head_commit_differs = true
"#,
            dir.path().display(),
            profile_path.display(),
            report_path.display()
        ),
    )
    .expect("write config");

    let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
    let store = BacklogStore::open(&db_path).expect("open store");
    for idx in 0..500 {
        upsert_task(&store, &format!("PTY task {idx}"));
    }

    let mut cmd = Command::new(&bin);
    cmd.arg("--config")
        .arg(&config_path)
        .arg("--working-dir")
        .arg(dir.path())
        .arg("--quit-after")
        .arg("500")
        .env("GARDENER_FORCE_TTY", "1")
        .env("GARDENER_DB_PATH", &db_path);
    (report_path, dir, store, cmd)
}

fn setup_live_interrupt_fixture() -> (TempDir, BacklogStore, Command) {
    let bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_gardener"));
    let dir = TempDir::new().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("mkdir bin");

    write_exec(
        &bin_dir.join("git"),
        r#"#!/bin/sh
set -e
if [ "$1" = "rev-parse" ] && [ "$2" = "--show-toplevel" ]; then
  pwd
  exit 0
fi
if [ "$1" = "rev-parse" ] && [ "$2" = "HEAD" ]; then
  echo deadbeef
  exit 0
fi
if [ "$1" = "worktree" ] && [ "$2" = "list" ]; then
  printf "worktree %s\nbranch refs/heads/main\n" "$(pwd)"
  exit 0
fi
if [ "$1" = "worktree" ] && [ "$2" = "add" ]; then
  mkdir -p "$3"
  exit 0
fi
if [ "$1" = "worktree" ] && [ "$2" = "remove" ]; then
  exit 0
fi
if [ "$1" = "worktree" ] && [ "$2" = "prune" ]; then
  exit 0
fi
exit 0
"#,
    );
    write_exec(
        &bin_dir.join("gh"),
        r#"#!/bin/sh
set -e
if [ "$1" = "pr" ] && [ "$2" = "list" ]; then
  echo "[]"
  exit 0
fi
exit 0
"#,
    );
    write_exec(
        &bin_dir.join("codex"),
        r#"#!/bin/sh
set -e
if [ "$1" = "--help" ]; then
  echo "Usage: codex --json --output-schema --output-last-message --listen stdio:// websocket"
  exit 0
fi
if [ "$1" = "--version" ]; then
  echo "codex 9.9.9"
  exit 0
fi
if [ "$1" = "exec" ]; then
  if printf "%s\n" "$@" | grep -q "Seed backlog tasks"; then
    :
  else
    sleep 5
  fi
  printf '{"type":"turn.completed","result":{"tasks":[],"branch":"feat/fsm","pr_number":12,"pr_url":"https://example.test/pr/12","verdict":"approve","suggestions":[],"merged":true,"merge_sha":"deadbeef"}}\n'
  exit 0
fi
exit 0
"#,
    );

    let config_path = dir.path().join("config.toml");
    let profile_path = dir.path().join(".gardener/repo-intelligence.toml");
    let report_path = dir.path().join(".gardener/quality.md");
    std::fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("mkdir profile");
    std::fs::write(
        &profile_path,
        include_str!("fixtures/triage/expected-profiles/phase03-profile.toml"),
    )
    .expect("write profile");
    std::fs::write(&report_path, "OLD_MARKER").expect("write report");
    std::fs::write(
        &config_path,
        format!(
            r#"[scope]
working_dir = "{}"

[validation]
command = "true"
allow_agent_discovery = true

[agent]
default = "codex"

[execution]
permissions_mode = "permissive_v1"
worker_mode = "normal"
test_mode = false

[orchestrator]
parallelism = 1

[startup]
validate_on_boot = false
validation_command = "true"

[triage]
output_path = "{}"
stale_after_commits = 50
discovery_max_turns = 12

[quality_report]
path = "{}"
stale_after_days = 7
stale_if_head_commit_differs = false

[seeding]
backend = "codex"
model = "gpt-5-codex"
max_turns = 12
"#,
            dir.path().display(),
            profile_path.display(),
            report_path.display()
        ),
    )
    .expect("write config");

    let db_path = dir.path().join(".cache/gardener/backlog.sqlite");
    let store = BacklogStore::open(&db_path).expect("open store");
    upsert_task(&store, "long running task");

    let mut cmd = Command::new(&bin);
    cmd.arg("--config")
        .arg(&config_path)
        .arg("--working-dir")
        .arg(dir.path())
        .arg("--quit-after")
        .arg("1")
        .env("GARDENER_FORCE_TTY", "1")
        .env("GARDENER_DB_PATH", &db_path)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
    (dir, store, cmd)
}

#[test]
fn pty_e2e_hotkeys_v_g_b_q_drive_screen_transitions() {
    let (report_path, _dir, store, cmd) = setup_pty_fixture();
    let mut session = expectrl::Session::spawn(cmd).expect("spawn pty");

    session.send("v").expect("send v");
    session.send("g").expect("send g");
    std::thread::sleep(Duration::from_millis(300));
    session.send("b").expect("send b");
    session.send("q").expect("send q");
    session.expect(Eof).expect("session exited");

    let report = std::fs::read_to_string(&report_path).expect("read report");
    assert!(
        !report.contains("OLD_MARKER"),
        "expected regenerate hotkey to rewrite quality report"
    );

    let tasks = store.list_tasks().expect("list tasks");
    let remaining = tasks
        .iter()
        .filter(|task| task.status != TaskStatus::Complete)
        .count();
    assert!(
        remaining > 0,
        "expected quit hotkey to stop run before finishing all seeded tasks"
    );
}

#[test]
fn pty_e2e_ctrl_c_quits() {
    let (_report_path, _dir, store, cmd) = setup_pty_fixture();
    let mut session = expectrl::Session::spawn(cmd).expect("spawn pty");
    session.send("\u{3}").expect("send ctrl-c");
    session.expect(Eof).expect("session exited");
    let tasks = store.list_tasks().expect("list tasks");
    let remaining = tasks
        .iter()
        .filter(|task| task.status != TaskStatus::Complete)
        .count();
    assert!(
        remaining > 0,
        "expected ctrl-c to stop run before finishing all seeded tasks"
    );
}

#[test]
fn pty_e2e_q_interrupts_live_blocking_turn() {
    let (_dir, store, cmd) = setup_live_interrupt_fixture();
    let mut session = expectrl::Session::spawn(cmd).expect("spawn pty");
    session.set_expect_timeout(Some(Duration::from_secs(8)));
    std::thread::sleep(Duration::from_millis(350));
    session.send("q").expect("send q");
    session.expect(Eof).expect("session exited");

    let tasks = store.list_tasks().expect("list tasks");
    let remaining = tasks
        .iter()
        .filter(|task| task.status != TaskStatus::Complete)
        .count();
    assert!(
        remaining > 0,
        "expected q to interrupt before all tasks complete"
    );
}

#[test]
fn pty_e2e_ctrl_c_interrupts_live_blocking_turn() {
    let (_dir, store, cmd) = setup_live_interrupt_fixture();
    let mut session = expectrl::Session::spawn(cmd).expect("spawn pty");
    session.set_expect_timeout(Some(Duration::from_secs(8)));
    std::thread::sleep(Duration::from_millis(350));
    session.send("\u{3}").expect("send ctrl-c");
    session.expect(Eof).expect("session exited");

    let tasks = store.list_tasks().expect("list tasks");
    let remaining = tasks
        .iter()
        .filter(|task| task.status != TaskStatus::Complete)
        .count();
    assert!(
        remaining > 0,
        "expected ctrl-c to interrupt before all tasks complete"
    );
}
