use assert_cmd::cargo::cargo_bin_cmd;

fn fixture(path: &str) -> String {
    format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn scheduler_mode_completes_target() {
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("--parallelism")
        .arg("3")
        .arg("--quit-after")
        .arg("3")
        .arg("--config")
        .arg(fixture("configs/phase04-scheduler-stub.toml"));
    let out = cmd.assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");
    assert!(stdout.contains("worker_id=pool state=complete"));
}
