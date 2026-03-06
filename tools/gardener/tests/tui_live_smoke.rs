use std::process::Command;

#[test]
fn live_tui_wrappers_run_under_a_pseudo_terminal() {
    let bin = env!("CARGO_BIN_EXE_tui_live_smoke");
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
            .args(["-qec", &format!("{bin} {mode}"), "/dev/null"])
            .status()
            .expect("spawn script");
        assert!(status.success(), "mode {mode} failed");
    }
}

#[test]
fn live_tui_wizards_run_under_a_pseudo_terminal() {
    let bin = env!("CARGO_BIN_EXE_tui_live_smoke");
    let seed_review = Command::new("sh")
        .env("TERM", "xterm")
        .args([
            "-c",
            &format!("printf 'kq' | script -qec '{bin} seed-review' /dev/null"),
        ])
        .status()
        .expect("spawn seed-review script");
    assert!(seed_review.success(), "seed-review mode failed");
}
