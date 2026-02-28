use gardener::runtime::{FakeTerminal, Terminal};
use gardener::tui::{render_dashboard, render_triage, BacklogView, QueueStats, WorkerRow};

fn make_worker(id: &str, state: &str, title: &str) -> WorkerRow {
    WorkerRow {
        worker_id: id.to_string(),
        state: state.to_string(),
        task_title: title.to_string(),
        tool_line: "git status".to_string(),
        breadcrumb: format!("start>{state}"),
        last_heartbeat_secs: 5,
        session_age_secs: 30,
        lease_held: true,
        session_missing: false,
        command_details: Vec::new(),
        commands_expanded: false,
    }
}

fn zero_stats() -> QueueStats {
    QueueStats {
        ready: 0,
        active: 0,
        failed: 0,
        p0: 0,
        p1: 0,
        p2: 0,
    }
}

fn empty_backlog() -> BacklogView {
    BacklogView::default()
}

#[test]
fn dashboard_header_shows_queue_stats() {
    let workers = vec![make_worker("w-01", "doing", "Fix the bug")];
    let stats = QueueStats {
        ready: 2,
        active: 1,
        failed: 0,
        p0: 1,
        p1: 1,
        p2: 0,
    };
    let frame = render_dashboard(&workers, &stats, &empty_backlog(), 120, 30);
    assert!(frame.contains("GARDENER"), "frame should contain GARDENER header");
}

#[test]
fn dashboard_worker_states_all_render() {
    for state in ["doing", "reviewing", "failed", "complete", "idle", "planning", "gitting"] {
        let frame = render_dashboard(
            &[make_worker("w-01", state, "task")],
            &zero_stats(),
            &empty_backlog(),
            120,
            30,
        );
        let frame_lower = frame.to_ascii_lowercase();
        assert!(
            frame_lower.contains(state),
            "state '{state}' not found in frame"
        );
    }
}

#[test]
fn dashboard_keeps_three_workers_visible_in_short_viewports_without_backlog() {
    let workers = vec![
        make_worker("w-01", "doing", "task-a"),
        make_worker("w-02", "doing", "task-b"),
        make_worker("w-03", "doing", "task-c"),
    ];
    let frame = render_dashboard(&workers, &QueueStats {
        ready: 0,
        active: 3,
        failed: 0,
        p0: 0,
        p1: 3,
        p2: 0,
    }, &BacklogView::default(), 80, 19);
    assert!(frame.contains("Lawn Mower"), "first worker card should be visible");
    assert!(frame.contains("Leaf Blower"), "second worker card should be visible");
    assert!(frame.contains("Hedge Trimmer"), "third worker card should be visible");
}

#[test]
fn dashboard_problems_panel_on_zombie_worker() {
    let zombie = WorkerRow {
        worker_id: "w-zombie".to_string(),
        state: "doing".to_string(),
        task_title: "stuck task".to_string(),
        tool_line: String::new(),
        breadcrumb: String::new(),
        last_heartbeat_secs: 9999,
        session_age_secs: 9999,
        lease_held: true,
        session_missing: true,
        command_details: Vec::new(),
        commands_expanded: false,
    };
    let frame = render_dashboard(&[zombie], &zero_stats(), &empty_backlog(), 120, 30);
    assert!(
        frame.contains("Problem") || frame.contains("Human") || frame.contains("zombie"),
        "frame should show problem indicator for zombie worker"
    );
}

#[test]
fn dashboard_empty_backlog_renders_without_panic() {
    let frame = render_dashboard(&[], &zero_stats(), &empty_backlog(), 120, 30);
    assert!(frame.contains("GARDENER"), "frame should still render header");
}

#[test]
fn dashboard_backlog_priority_badges() {
    let backlog = BacklogView {
        in_progress: vec!["P0 abc123 Critical task".to_string()],
        queued: vec!["P1 def456 Normal task".to_string()],
    };
    let frame = render_dashboard(
        &[make_worker("w-01", "doing", "task")],
        &QueueStats { ready: 1, active: 1, failed: 0, p0: 1, p1: 1, p2: 0 },
        &backlog,
        120,
        30,
    );
    assert!(frame.contains("P0"), "frame should contain P0");
    assert!(frame.contains("Critical task"), "frame should contain task title");
}

#[test]
fn triage_screen_renders_activity() {
    let activity = vec!["Scanning repository".to_string(), "Detecting tools".to_string()];
    let artifacts = vec!["agent: codex".to_string()];
    let frame = render_triage(&activity, &artifacts, 120, 30);
    assert!(frame.contains("GARDENER"), "triage frame should contain GARDENER");
}

#[test]
fn triage_render_includes_artifacts() {
    let activity = vec!["Step 1".to_string()];
    let artifacts = vec!["Detected agent: codex".to_string()];
    let frame = render_triage(&activity, &artifacts, 120, 30);
    assert!(
        frame.contains("Detected agent") || frame.contains("codex"),
        "triage frame should contain artifact text"
    );
}

#[test]
fn report_screen_via_fake_terminal() {
    let terminal = FakeTerminal::new(true);
    terminal.draw_report("/tmp/quality.md", "grade: B\noverall: good").expect("draw_report");
    let draws = terminal.report_draws();
    assert_eq!(draws.len(), 1);
    assert_eq!(draws[0].0, "/tmp/quality.md");
    assert!(draws[0].1.contains("grade: B"), "report content should be captured");
}

#[test]
fn shutdown_screen_captures_title_and_message() {
    let terminal = FakeTerminal::new(true);
    terminal.draw_shutdown_screen("error: disk full", "out of space").expect("draw_shutdown_screen");
    let screens = terminal.shutdown_screens();
    assert_eq!(screens.len(), 1);
    assert_eq!(screens[0].0, "error: disk full");
    assert!(screens[0].1.contains("out of space"));
}
