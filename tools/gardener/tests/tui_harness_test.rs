use gardener::logging::structured_fallback_line;
use gardener::runtime::{FakeTerminal, Terminal};
use gardener::tui::{handle_key, render_dashboard, QueueStats, WorkerRow};

#[test]
fn fake_terminal_captures_tui_frames_and_interactions() {
    let terminal = FakeTerminal::new(true);

    let frame = render_dashboard(
        &[WorkerRow {
            worker_id: "worker-1".to_string(),
            state: "doing".to_string(),
            tool_line: "git status".to_string(),
            breadcrumb: "understand>doing".to_string(),
            last_heartbeat_secs: 5,
            session_age_secs: 30,
            lease_held: true,
            session_missing: false,
        }],
        &QueueStats {
            ready: 2,
            active: 1,
            failed: 0,
            p0: 1,
            p1: 1,
            p2: 0,
        },
        80,
        18,
    );

    terminal.draw(&frame).expect("draw");
    terminal
        .write_line(handle_key('q'))
        .expect("interaction command");

    let frames = terminal.drawn_frames();
    assert_eq!(frames.len(), 1);
    assert!(frames[0].contains("worker-1"));

    let lines = terminal.written_lines();
    assert_eq!(lines, vec!["quit".to_string()]);
}

#[test]
fn non_tty_fallback_line_is_stable_in_harness() {
    let line = structured_fallback_line("worker-1", "reviewing", "tool call");
    assert_eq!(
        line,
        "worker_id=worker-1 state=reviewing message=tool call "
    );
}
