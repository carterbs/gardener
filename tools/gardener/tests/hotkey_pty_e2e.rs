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
            scope_key: "scope".to_string(),
            priority: Priority::P1,
            source: "pty-test".to_string(),
            related_pr: None,
            related_branch: None,
        })
        .expect("upsert task");
}

#[test]
fn pty_e2e_hotkeys_v_g_b_q_drive_screen_transitions() {
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

    let store =
        BacklogStore::open(dir.path().join(".cache/gardener/backlog.sqlite")).expect("open store");
    for idx in 0..500 {
        upsert_task(&store, &format!("PTY task {idx}"));
    }

    let mut cmd = Command::new(bin);
    cmd.arg("--config")
        .arg(&config_path)
        .arg("--working-dir")
        .arg(dir.path())
        .arg("--quit-after")
        .arg("500")
        .env("GARDENER_FORCE_TTY", "1");
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
