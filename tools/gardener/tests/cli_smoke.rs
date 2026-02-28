use assert_cmd::cargo::cargo_bin_cmd;

fn fixture(path: &str) -> String {
    format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn help_lists_phase1_flags() {
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--help");
    let out = cmd.assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");

    assert!(stdout.contains("--agent"));
    assert!(stdout.contains("--quit-after"));
    assert!(!stdout.contains("--headless"));
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
fn sync_only_exports_snapshot_and_exits_zero() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--sync-only")
        .arg("--config")
        .arg(fixture("configs/phase09-cutover.toml"))
        .arg("--working-dir")
        .arg(temp.path());
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
