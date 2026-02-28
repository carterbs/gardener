use gardener::logging::structured_fallback_line;
use gardener::runtime::{FakeTerminal, Terminal};
use gardener::tui::{render_dashboard, BacklogView, QueueStats, WorkerRow};

#[test]
fn fake_terminal_captures_tui_frames_and_interactions() {
    let terminal = FakeTerminal::new(true);

    let frame = render_dashboard(
        &[WorkerRow {
            worker_id: "worker-1".to_string(),
            state: "doing".to_string(),
            task_title: "Implement queue panel".to_string(),
            tool_line: "git status".to_string(),
            breadcrumb: "understand>doing".to_string(),
            last_heartbeat_secs: 5,
            session_age_secs: 30,
            lease_held: true,
            session_missing: false,
            command_details: Vec::new(),
            commands_expanded: false,
        }],
        &QueueStats {
            ready: 2,
            active: 1,
            failed: 0,
            p0: 1,
            p1: 1,
            p2: 0,
        },
        &BacklogView {
            in_progress: vec!["P1 abc123 implement worker loop".to_string()],
            queued: vec![
                "P0 deadbe unblock ci".to_string(),
                "P2 cafe00 cleanup docs".to_string(),
            ],
        },
        80,
        18,
    );

    terminal.draw(&frame).expect("draw");

    let frames = terminal.drawn_frames();
    assert_eq!(frames.len(), 1);
    assert!(frames[0].contains("Lawn Mower"));
}

#[test]
fn non_tty_fallback_line_is_stable_in_harness() {
    let line = structured_fallback_line("worker-1", "reviewing", "tool call");
    assert_eq!(
        line,
        "worker_id=worker-1 state=reviewing message=tool call "
    );
}
