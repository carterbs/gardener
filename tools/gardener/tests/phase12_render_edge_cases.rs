use gardener::runtime::{FakeFileSystem, FileSystem, FakeTerminal, Terminal};
use std::path::Path;
use gardener::tui::{render_dashboard, render_report_view, render_triage, BacklogView, QueueStats, WorkerRow};

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
fn render_dashboard_zero_width_zero_height() {
    let frame = render_dashboard(&[], &zero_stats(), &empty_backlog(), 0, 0);
    assert!(frame.is_empty());
}

#[test]
fn render_dashboard_width_1_height_1() {
    let frame = render_dashboard(
        &[make_worker("w-01", "doing", "task")],
        &zero_stats(),
        &empty_backlog(),
        1,
        1,
    );
    assert!(!frame.is_empty());
}

#[test]
fn render_dashboard_many_workers_small_viewport() {
    let workers: Vec<_> = (0..50)
        .map(|i| make_worker(&format!("w-{i:02}"), "doing", &format!("task {i:02}")))
        .collect();
    let frame = render_dashboard(&workers, &zero_stats(), &empty_backlog(), 120, 10);
    assert!(frame.contains("task 00"));
    assert!(!frame.contains("task 49"));
}

#[test]
fn render_triage_empty_activity_no_panic() {
    let frame = render_triage(&[], &[], 120, 30);
    assert!(frame.contains("GARDENER"));
}

#[test]
fn render_report_view_empty_report() {
    let frame = render_report_view("/tmp/quality.md", "", 120, 30);
    assert!(frame.contains("Quality report view"));
}

#[test]
fn render_report_view_very_long_content_truncated() {
    let report = (0..1000).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let frame = render_report_view("/tmp/report.md", &report, 120, 30);
    assert!(frame.contains("line 0"));
    assert!(!frame.contains("line 999"));
}

#[test]
fn fake_terminal_draw_shutdown_does_not_produce_drawn_frame() {
    let terminal = FakeTerminal::new(true);
    terminal.draw_shutdown_screen("title", "message").expect("draw shutdown");
    assert!(terminal.drawn_frames().is_empty());
    assert_eq!(terminal.shutdown_screens().len(), 1);
}

#[test]
fn fake_filesystem_exists_does_not_see_directories() {
    let fs = FakeFileSystem::default();
    let dir = Path::new("/some/dir");
    fs.create_dir_all(dir).expect("mkdir");
    assert!(!fs.exists(dir));
}
