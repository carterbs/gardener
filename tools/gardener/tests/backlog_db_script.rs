use std::path::PathBuf;
use std::process::Command;

fn backlog_db_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
        .join("backlog-db.sh")
}

#[test]
fn backlog_db_help_includes_runbook_command() {
    let output = Command::new("bash")
        .arg(backlog_db_script())
        .arg("help")
        .output()
        .expect("run backlog-db help");

    assert!(
        output.status.success(),
        "help command should exit successfully"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("scripts/backlog-db.sh runbook"),
        "runbook command should be listed in help output"
    );
}

#[test]
fn backlog_db_runbook_command_prints_markdown() {
    let output = Command::new("bash")
        .arg(backlog_db_script())
        .arg("runbook")
        .output()
        .expect("run backlog-db runbook");

    assert!(
        output.status.success(),
        "runbook command should exit successfully"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("# Backlog Operations Runbook for Agents"),
        "runbook output should include top-level heading"
    );
}

#[test]
fn backlog_db_add_rejects_invalid_priority() {
    let output = Command::new("bash")
        .arg(backlog_db_script())
        .args([
            "add",
            "--title",
            "bad priority",
            "--details",
            "d",
            "--priority",
            "P9",
        ])
        .output()
        .expect("run backlog-db add with invalid priority");

    assert!(
        !output.status.success(),
        "invalid priority should make add command fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid --priority"),
        "stderr should report invalid priority value"
    );
}

#[test]
fn backlog_db_add_rejects_invalid_status() {
    let output = Command::new("bash")
        .arg(backlog_db_script())
        .args([
            "add",
            "--title",
            "bad status",
            "--details",
            "d",
            "--status",
            "busy",
        ])
        .output()
        .expect("run backlog-db add with invalid status");

    assert!(
        !output.status.success(),
        "invalid status should make add command fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid --status"),
        "stderr should report invalid status value"
    );
}

#[test]
fn backlog_db_add_rejects_invalid_kind() {
    let output = Command::new("bash")
        .arg(backlog_db_script())
        .args([
            "add",
            "--title",
            "bad kind",
            "--details",
            "d",
            "--kind",
            "QualityGap",
        ])
        .output()
        .expect("run backlog-db add with invalid kind");

    assert!(
        !output.status.success(),
        "invalid kind should make add command fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid --kind"),
        "stderr should report invalid kind value"
    );
}
