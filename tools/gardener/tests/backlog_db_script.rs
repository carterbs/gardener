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
fn backlog_db_help_shows_default_db_path() {
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
        stdout.contains(".cache/gardener/backlog.sqlite"),
        "help output should mention canonical default db path"
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
