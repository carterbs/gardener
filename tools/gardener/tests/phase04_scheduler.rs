use assert_cmd::Command;

fn fixture(path: &str) -> String {
    format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn scheduler_stub_mode_completes_target_without_fsm() {
    let mut cmd = Command::cargo_bin("gardener").expect("bin");
    cmd.arg("--parallelism")
        .arg("3")
        .arg("--target")
        .arg("3")
        .arg("--config")
        .arg(fixture("configs/phase04-scheduler-stub.toml"));
    let out = cmd.assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");
    assert!(stdout.contains("phase4 scheduler-stub complete"));
}
