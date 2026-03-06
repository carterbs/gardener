use assert_cmd::cargo::cargo_bin_cmd;
use gardener::backlog_store::{BacklogStore, ManualTaskInput, TaskStatus};
use gardener::priority::Priority;
use gardener::task_identity::TaskKind;
use serde_json::Value;
use tempfile::{tempdir, TempDir};

fn temp_store() -> (BacklogStore, TempDir, std::path::PathBuf) {
    let dir = tempdir().expect("tempdir");
    let db = dir.path().join("backlog.sqlite");
    let store = BacklogStore::open(&db).expect("open store");
    (store, dir, db)
}

fn seed_manual_task(store: &BacklogStore, id: &str, title: &str) {
    store
        .insert_manual_task(ManualTaskInput {
            task_id: id.to_string(),
            kind: TaskKind::Feature,
            title: title.to_string(),
            details: format!("details for {title}"),
            rationale: String::new(),
            scope_key: "runtime".to_string(),
            priority: Priority::P1,
            status: TaskStatus::Ready,
            source: "test".to_string(),
            related_pr: None,
            related_branch: None,
        })
        .expect("seed task");
}

#[test]
fn list_shows_recent_rows() {
    let (store, _dir, db) = temp_store();
    seed_manual_task(&store, "manual:runtime:auto-1", "first task");
    std::thread::sleep(std::time::Duration::from_millis(5));
    seed_manual_task(&store, "manual:runtime:auto-2", "second task");

    let output = cargo_bin_cmd!("backlog-db")
        .args(["list", "--db", db.to_str().expect("path")])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("stdout");
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("manual:runtime:auto-2|second task|P1|ready|test|runtime"));
    assert!(lines[1].starts_with("manual:runtime:auto-1|first task|P1|ready|test|runtime"));
}

#[test]
fn add_creates_manual_row() {
    let (_store, _dir, db) = temp_store();

    let output = cargo_bin_cmd!("backlog-db")
        .args([
            "add",
            "--db",
            db.to_str().expect("path"),
            "--title",
            "manual task",
            "--details",
            "details",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("stdout");
    assert!(stdout.contains("created: manual:runtime:auto-"));

    let store = BacklogStore::open(&db).expect("reopen store");
    let rows = store.list_recent_tasks(10).expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "manual task");
}

#[test]
fn add_respects_custom_id() {
    let (_store, _dir, db) = temp_store();

    cargo_bin_cmd!("backlog-db")
        .args([
            "add",
            "--db",
            db.to_str().expect("path"),
            "--title",
            "manual task",
            "--details",
            "details",
            "--id",
            "manual:runtime:custom-id",
        ])
        .assert()
        .success();

    let store = BacklogStore::open(&db).expect("reopen store");
    let row = store
        .get_task("manual:runtime:custom-id")
        .expect("get task")
        .expect("task");
    assert_eq!(row.title, "manual task");
}

#[test]
fn add_rejects_invalid_priority() {
    let (_store, _dir, db) = temp_store();

    let output = cargo_bin_cmd!("backlog-db")
        .args([
            "add",
            "--db",
            db.to_str().expect("path"),
            "--title",
            "bad priority",
            "--details",
            "details",
            "--priority",
            "P9",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8(output).expect("stderr");
    assert!(stderr.contains("invalid --priority"));
}

#[test]
fn add_rejects_invalid_status() {
    let (_store, _dir, db) = temp_store();

    let output = cargo_bin_cmd!("backlog-db")
        .args([
            "add",
            "--db",
            db.to_str().expect("path"),
            "--title",
            "bad status",
            "--details",
            "details",
            "--status",
            "busy",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8(output).expect("stderr");
    assert!(stderr.contains("invalid --status"));
}

#[test]
fn add_rejects_invalid_kind() {
    let (_store, _dir, db) = temp_store();

    let output = cargo_bin_cmd!("backlog-db")
        .args([
            "add",
            "--db",
            db.to_str().expect("path"),
            "--title",
            "bad kind",
            "--details",
            "details",
            "--kind",
            "QualityGap",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8(output).expect("stderr");
    assert!(stderr.contains("invalid --kind"));
}

#[test]
fn add_requires_title_and_details() {
    let (_store, _dir, db) = temp_store();

    let output = cargo_bin_cmd!("backlog-db")
        .args(["add", "--db", db.to_str().expect("path"), "--details", "details"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8(output).expect("stderr");
    assert!(stderr.contains("--title and --details are required for add"));
}

#[test]
fn runbook_prints_markdown() {
    let output = cargo_bin_cmd!("backlog-db")
        .arg("runbook")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("stdout");
    assert!(stdout.contains("# Backlog Operations Runbook for Agents"));
}

#[test]
fn show_json_returns_task() {
    let (store, _dir, db) = temp_store();
    seed_manual_task(&store, "manual:runtime:auto-1", "show task");

    let output = cargo_bin_cmd!("backlog-db")
        .args([
            "show",
            "--db",
            db.to_str().expect("path"),
            "--id",
            "manual:runtime:auto-1",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(parsed["task_id"], "manual:runtime:auto-1");
    assert_eq!(parsed["title"], "show task");
}

#[test]
fn update_json_returns_before_and_after() {
    let (store, _dir, db) = temp_store();
    seed_manual_task(&store, "manual:runtime:auto-1", "update task");

    let output = cargo_bin_cmd!("backlog-db")
        .args([
            "update",
            "--db",
            db.to_str().expect("path"),
            "--id",
            "manual:runtime:auto-1",
            "--status",
            "complete",
            "--rationale",
            "manually closed",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(parsed["before"]["status"], "ready");
    assert_eq!(parsed["after"]["status"], "complete");
    assert_eq!(parsed["after"]["rationale"], "manually closed");
}

#[test]
fn update_dry_run_does_not_persist_changes() {
    let (store, _dir, db) = temp_store();
    seed_manual_task(&store, "manual:runtime:auto-1", "dry run task");

    let output = cargo_bin_cmd!("backlog-db")
        .args([
            "update",
            "--db",
            db.to_str().expect("path"),
            "--id",
            "manual:runtime:auto-1",
            "--status",
            "complete",
            "--rationale",
            "preview only",
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(parsed["after"]["status"], "complete");

    let task = BacklogStore::open(&db)
        .expect("reopen")
        .get_task("manual:runtime:auto-1")
        .expect("get task")
        .expect("task");
    assert_eq!(task.status, TaskStatus::Ready);
    assert_eq!(task.rationale, "");
}

#[test]
fn update_clear_lease_removes_owner() {
    let (store, _dir, db) = temp_store();
    seed_manual_task(&store, "manual:runtime:auto-1", "leased task");
    let claimed = store
        .claim_next("worker-a", 60)
        .expect("claim")
        .expect("claimed task");
    assert_eq!(claimed.task_id, "manual:runtime:auto-1");

    cargo_bin_cmd!("backlog-db")
        .args([
            "update",
            "--db",
            db.to_str().expect("path"),
            "--id",
            "manual:runtime:auto-1",
            "--status",
            "ready",
            "--clear-lease",
        ])
        .assert()
        .success();

    let task = BacklogStore::open(&db)
        .expect("reopen")
        .get_task("manual:runtime:auto-1")
        .expect("get task")
        .expect("task");
    assert_eq!(task.status, TaskStatus::Ready);
    assert_eq!(task.lease_owner, None);
    assert_eq!(task.lease_expires_at, None);
}

#[test]
fn retire_sets_final_status() {
    let (store, _dir, db) = temp_store();
    seed_manual_task(&store, "manual:runtime:auto-1", "retire task");

    cargo_bin_cmd!("backlog-db")
        .args([
            "retire",
            "--db",
            db.to_str().expect("path"),
            "--id",
            "manual:runtime:auto-1",
            "--status",
            "failed",
            "--rationale",
            "duplicate",
        ])
        .assert()
        .success();

    let store = BacklogStore::open(&db).expect("reopen");
    let task = store
        .get_task("manual:runtime:auto-1")
        .expect("get task")
        .expect("task");
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.rationale, "duplicate");
}

#[test]
fn retire_clears_existing_lease_without_explicit_flag() {
    let (store, _dir, db) = temp_store();
    seed_manual_task(&store, "manual:runtime:auto-1", "retire leased task");
    let claimed = store
        .claim_next("worker-a", 60)
        .expect("claim")
        .expect("claimed task");
    assert_eq!(claimed.task_id, "manual:runtime:auto-1");

    cargo_bin_cmd!("backlog-db")
        .args([
            "retire",
            "--db",
            db.to_str().expect("path"),
            "--id",
            "manual:runtime:auto-1",
            "--status",
            "failed",
            "--rationale",
            "duplicate",
        ])
        .assert()
        .success();

    let task = BacklogStore::open(&db)
        .expect("reopen")
        .get_task("manual:runtime:auto-1")
        .expect("get task")
        .expect("task");
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.lease_owner, None);
    assert_eq!(task.lease_expires_at, None);
}
