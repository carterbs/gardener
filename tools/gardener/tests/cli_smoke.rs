use assert_cmd::Command;

fn fixture(path: &str) -> String {
    format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn help_lists_phase1_flags() {
    let mut cmd = Command::cargo_bin("gardener").expect("bin");
    cmd.arg("--help");
    let out = cmd.assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");

    assert!(stdout.contains("--agent"));
    assert!(!stdout.contains("--headless"));
}

#[test]
fn prune_only_smoke_succeeds() {
    let mut cmd = Command::cargo_bin("gardener").expect("bin");
    cmd.arg("--prune-only")
        .arg("--config")
        .arg(fixture("configs/phase01-minimal.toml"));
    cmd.assert().success();
}

#[test]
fn prune_only_with_scoped_working_dir_succeeds() {
    let mut cmd = Command::cargo_bin("gardener").expect("bin");
    cmd.arg("--prune-only")
        .arg("--config")
        .arg(fixture("configs/phase01-minimal.toml"))
        .arg("--working-dir")
        .arg(fixture("repos/scoped-app/packages/functions/src"));
    cmd.assert().success();
}

#[test]
fn invalid_config_path_exits_nonzero() {
    let mut cmd = Command::cargo_bin("gardener").expect("bin");
    cmd.arg("--prune-only")
        .arg("--config")
        .arg(fixture("configs/missing.toml"));
    cmd.assert().failure();
}
