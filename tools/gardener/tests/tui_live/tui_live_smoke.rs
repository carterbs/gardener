use expectrl::{Eof, Expect};
use std::process::Command;
use std::time::Duration;

#[test]
fn live_tui_wrappers_run_under_a_pseudo_terminal() {
    let bin = env!("CARGO_BIN_EXE_gardener");
    let log_path = std::env::temp_dir().join("gardener-tui-live-smoke-otel-logs.jsonl");

    for mode in [
        "dashboard",
        "report",
        "seeding",
        "triage",
        "shutdown",
        "quality-grading",
        "quality-intro",
    ] {
        let status = Command::new("script")
            .env("TERM", "xterm")
            .env("GARDENER_LOG_PATH", &log_path)
            .args(["-qec", &format!("{bin} tui-live-smoke {mode}"), "/dev/null"])
            .status()
            .expect("spawn script");
        assert!(status.success(), "mode {mode} failed");
    }
}

#[test]
fn live_tui_wizards_run_under_a_pseudo_terminal() {
    let mut wizard_cmd = Command::new(env!("CARGO_BIN_EXE_gardener"));
    wizard_cmd
        .arg("tui-live-smoke")
        .arg("wizard")
        .env("TERM", "xterm")
        .env(
            "GARDENER_LOG_PATH",
            std::env::temp_dir().join("gardener-tui-live-smoke-otel-logs.jsonl"),
        )
        .env("GARDENER_FORCE_TTY", "1");
    let mut wizard = expectrl::Session::spawn(wizard_cmd).expect("spawn wizard pty");
    wizard.set_expect_timeout(Some(Duration::from_secs(10)));
    wizard.send("\u{1b}").expect("send wizard escape");
    wizard.expect(Eof).expect("wizard exited");

    let mut seed_review_cmd = Command::new(env!("CARGO_BIN_EXE_gardener"));
    seed_review_cmd
        .arg("tui-live-smoke")
        .arg("seed-review")
        .env("TERM", "xterm")
        .env(
            "GARDENER_LOG_PATH",
            std::env::temp_dir().join("gardener-tui-live-smoke-otel-logs.jsonl"),
        )
        .env("GARDENER_FORCE_TTY", "1");
    let mut seed_review =
        expectrl::Session::spawn(seed_review_cmd).expect("spawn seed-review pty");
    seed_review.set_expect_timeout(Some(Duration::from_secs(10)));
    seed_review.send("k").expect("send keep");
    seed_review.send("q").expect("send quit");
    seed_review.expect(Eof).expect("seed-review exited");
}
