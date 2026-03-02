use assert_cmd::cargo::cargo_bin_cmd;

fn fixture(path: &str) -> String {
    format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))
}

fn workflow_termination_flags() -> Vec<&'static str> {
    let docs = include_str!("../../../docs/conventions/workflow.md");
    let mut in_section = false;
    let mut flags = Vec::new();

    for line in docs.lines() {
        if line.trim() == "## Termination Modes" {
            in_section = true;
            continue;
        }

        if in_section && line.starts_with("## ") {
            break;
        }

        if !in_section {
            continue;
        }

        let line = line.trim_start();
        if let Some(flag_text) = line.strip_prefix("- `") {
            if let Some(flag_end) = flag_text.find('`') {
                let flag_text = &flag_text[..flag_end];
                let flag = flag_text.split_whitespace().next().unwrap_or(flag_text);
                if flag.starts_with("--") {
                    flags.push(flag);
                }
            }
        }
    }

    flags
}

#[test]
fn help_lists_phase1_flags() {
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--help");
    let out = cmd.assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");

    assert!(stdout.contains("--agent"));
    assert!(stdout.contains("--worker-mode"));
    assert!(stdout.contains("--quit-after"));
    assert!(!stdout.contains("--headless"));
}

#[test]
fn documented_termination_flags_are_listed_in_help() {
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--help");
    let out = cmd.assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");

    for flag in workflow_termination_flags() {
        assert!(stdout.contains(flag), "expected help to contain {flag}");
    }
}

#[test]
fn prune_only_smoke_succeeds() {
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--prune-only")
        .arg("--config")
        .arg(fixture("configs/phase01-minimal.toml"));
    cmd.assert().success();
}

#[test]
fn prune_only_with_scoped_working_dir_succeeds() {
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--prune-only")
        .arg("--config")
        .arg(fixture("configs/phase01-minimal.toml"))
        .arg("--working-dir")
        .arg(fixture("repos/scoped-app/packages/functions/src"));
    cmd.assert().success();
}

#[test]
fn quit_after_smoke_succeeds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--quit-after")
        .arg("0")
        .arg("--config")
        .arg(fixture("configs/phase01-minimal.toml"))
        .env("GARDENER_DB_PATH", temp.path().join("backlog.sqlite"));
    cmd.assert().success();
}

#[test]
fn sync_only_exports_snapshot_and_exits_zero() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--sync-only")
        .arg("--config")
        .arg(fixture("configs/phase09-cutover.toml"))
        .arg("--working-dir")
        .arg(temp.path())
        .env("GARDENER_DB_PATH", temp.path().join("backlog.sqlite"));
    let out = cmd.assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");
    assert!(stdout.contains("sync complete: snapshot="));
}

#[test]
fn invalid_config_path_exits_nonzero() {
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--prune-only")
        .arg("--config")
        .arg(fixture("configs/missing.toml"));
    cmd.assert().failure();
}

#[test]
fn seed_backlog_invalid_config_path_exits_nonzero() {
    let mut cmd = cargo_bin_cmd!("seed-backlog");
    cmd.arg("--config").arg(fixture("configs/missing.toml"));
    cmd.assert().failure();
}
