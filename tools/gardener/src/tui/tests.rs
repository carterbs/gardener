use super::{
    command_stream_window, format_breadcrumb, format_state_label, render_dashboard,
    render_dashboard_at_tick, render_triage, reset_workers_scroll, scroll_workers_down,
    scroll_workers_up, worker_command_stream, worker_flow_chain_spans, AppState, BacklogView,
    CommandEntry, QueueStats, StageState, StartupHeadlineView, WorkerCard, WorkerMetrics,
    WorkerRow, WorkerState,
};

fn worker(heartbeat: u64, missing: bool) -> WorkerRow {
    WorkerRow {
        worker_id: "w1".to_string(),
        state: "doing".to_string(),
        task_id: None,
        last_state_line: 0,
        task_title: "task: demo".to_string(),
        tool_line: "rg --files".to_string(),
        breadcrumb: "understand>doing".to_string(),
        last_heartbeat_secs: heartbeat,
        session_age_secs: 33,
        lease_held: true,
        session_missing: missing,
        command_details: Vec::new(),
    }
}

#[test]
fn render_and_key_handling_cover_ui_branches() {
    let frame = render_dashboard(
        &[worker(10, false)],
        &QueueStats {
            ready: 1,
            active: 1,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 1,
            p1: 0,
            p2: 0,
        },
        &BacklogView {
            in_progress: vec!["INP P1 fix queue".to_string()],
            queued: vec![
                "P0 abc123 queued task".to_string(),
                "P2 def456 tune logs".to_string(),
            ],
        },
        80,
        40,
    );
    assert!(frame.contains("GARDENER"));
    assert!(frame.contains("Now"));
    assert!(frame.contains("Scanning"));
    assert!(frame.contains("parallel workers"));
    assert!(!frame.contains("Workers:"));
    assert!(!frame.contains("Problems"));
    assert!(frame.contains("Flow:"));
    assert!(frame.contains("Action:"));
    assert!(frame.contains("P0"));
    assert!(frame.contains("P2"));
    assert!(!frame.contains("fix queue"));
    assert!(!frame.contains("status="));
    assert!(!frame.contains("action="));
}

#[test]
fn backlog_rendering_is_priority_ordered() {
    let frame = render_dashboard(
        &[worker(10, false)],
        &QueueStats {
            ready: 1,
            active: 1,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 1,
            p1: 2,
            p2: 2,
        },
        &BacklogView {
            in_progress: vec![
                "INP P1 active task should be omitted".to_string(),
                "INP P0 bravo task".to_string(),
                "INP P2 charlie task".to_string(),
            ],
            queued: vec![
                "P0 abc123 queued p0".to_string(),
                "P1 def456 queued p1".to_string(),
                "P2 feed00 queued p2".to_string(),
            ],
        },
        80,
        40,
    );
    let backlog_section_start = frame
        .find("BACKLOG (PRIORITY ORDER)")
        .expect("backlog heading");
    let backlog_section = &frame[backlog_section_start..];
    let p0 = backlog_section.find("queued p0").expect("p0 row");
    let p1 = backlog_section.find("queued p1").expect("p1 row");
    let p2 = backlog_section.find("queued p2").expect("p2 row");
    assert!(
        p0 < p2,
        "P0 rows should render before P2 rows in Backlog panel"
    );
    assert!(
        p0 < p1,
        "P0 rows should render before P1 rows in Backlog panel"
    );
    assert!(
        !backlog_section.contains("active task should be omitted"),
        "INP items should be excluded from backlog panel"
    );
    assert!(
        !backlog_section.contains("bravo task"),
        "INP items should be excluded from backlog panel"
    );
}

#[test]
fn backlog_excludes_in_progress_tasks() {
    let frame = render_dashboard(
        &[worker(10, false)],
        &QueueStats {
            ready: 1,
            active: 1,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 1,
            p1: 1,
            p2: 0,
        },
        &BacklogView {
            in_progress: vec!["INP P1 5d8c91 active task".to_string()],
            queued: vec!["P0 abc123 queued task".to_string()],
        },
        120,
        30,
    );
    assert!(frame.contains("queued task"));
    assert!(!frame.contains("P1 active task"));
}

#[test]
fn dashboard_panes_render_with_borders() {
    let frame = render_dashboard(
        &[worker(10, false)],
        &QueueStats {
            ready: 1,
            active: 1,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 1,
            p1: 0,
            p2: 0,
        },
        &BacklogView {
            in_progress: vec!["P1 abc123 fix queue".to_string()],
            queued: vec!["P2 def456 tune logs".to_string()],
        },
        120,
        30,
    );
    let border_chars = |line: &str| {
        line.chars().any(|ch| {
            matches!(
                ch,
                '─' | '│'
                    | '┌'
                    | '┐'
                    | '└'
                    | '┘'
                    | '╭'
                    | '╮'
                    | '╰'
                    | '╯'
                    | '+'
                    | '┬'
                    | '┴'
                    | '├'
                    | '┤'
            )
        })
    };
    let has_title_with_border = |frame: &str, title: &str| {
        frame
            .lines()
            .any(|line| line.contains(title) && border_chars(line))
    };
    let top_left_corners =
        frame.matches('┌').count() + frame.matches('╭').count() + frame.matches('+').count();
    let top_right_corners =
        frame.matches('┐').count() + frame.matches('╮').count() + frame.matches('+').count();
    assert!(
        top_left_corners >= 3,
        "expected now/backlog/merge queue borders"
    );
    assert!(
        top_right_corners >= 3,
        "expected now/backlog/merge queue/nows borders"
    );
    assert!(has_title_with_border(&frame, "Backlog"));
    assert!(has_title_with_border(&frame, "Merge Queue"));
    assert!(frame.contains("Backlog"));
    assert!(frame.contains("Merge Queue"));
}

#[test]
fn work_now_card_freezes_spinner_after_startup() {
    let active_frame = render_dashboard_at_tick(
        &[worker(10, false)],
        &QueueStats {
            ready: 1,
            active: 1,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 0,
            p1: 1,
            p2: 0,
        },
        &BacklogView::default(),
        90,
        22,
        5,
        2,
    );
    let frozen_frame = render_dashboard_at_tick(
        &[worker(10, false)],
        &QueueStats {
            ready: 1,
            active: 1,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 0,
            p1: 1,
            p2: 0,
        },
        &BacklogView::default(),
        90,
        22,
        35,
        2,
    );
    assert!(active_frame.contains("Pruning"));
    assert!(frozen_frame.contains("Pruning"));
    assert!(frozen_frame.contains("..."));
    assert!(active_frame.contains("⠇"));
    assert!(frozen_frame.contains("⠇"));
}

#[test]
fn does_not_render_human_problem_panel() {
    let frame = render_dashboard(
        &[worker(901, false)],
        &QueueStats {
            ready: 0,
            active: 1,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 1,
            p1: 0,
            p2: 0,
        },
        &BacklogView::default(),
        80,
        20,
    );
    assert!(!frame.contains("Problems Requiring Human"));
    assert!(!frame.contains("needs intervention"));
}

#[test]
fn dashboard_worker_labels_are_readable() {
    let frame = render_dashboard(
        &[
            WorkerRow {
                worker_id: "w1".to_string(),
                state: "backlog_sync".to_string(),
                task_id: None,
                last_state_line: 0,
                task_title: "task one".to_string(),
                tool_line: "tool".to_string(),
                breadcrumb: "boot>backlog_sync".to_string(),
                last_heartbeat_secs: 5,
                session_age_secs: 1,
                lease_held: true,
                session_missing: false,
                command_details: Vec::new(),
            },
            WorkerRow {
                worker_id: "w2".to_string(),
                state: "merging".to_string(),
                task_id: Some("task-two".to_string()),
                last_state_line: 0,
                task_title: "task two".to_string(),
                tool_line: "prompt 12".to_string(),
                breadcrumb: "state>merging".to_string(),
                last_heartbeat_secs: 5,
                session_age_secs: 1,
                lease_held: true,
                session_missing: false,
                command_details: Vec::new(),
            },
        ],
        &QueueStats {
            ready: 0,
            active: 2,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 0,
            p1: 2,
            p2: 0,
        },
        &BacklogView::default(),
        120,
        30,
    );
    assert_eq!(
        format_breadcrumb("boot>backlog_sync"),
        "Boot > Backlog Sync"
    );
    assert_eq!(format_breadcrumb("state>merging"), "Merging");
    assert_eq!(format_state_label("backlog_sync"), "Backlog Sync");
    assert_eq!(format_state_label("merging"), "Merging");
    assert!(frame.contains("task one"));
    assert!(frame.contains("task two"));
    assert!(frame.contains("Flow:"));
}

#[test]
fn worker_flow_chain_shows_full_chain_for_understand_state() {
    let spans = worker_flow_chain_spans("understand");
    let labels = spans
        .iter()
        .map(|span| span.content.to_string())
        .filter(|label| label != " → ")
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            "Understand",
            "Planning",
            "Doing",
            "Gitting",
            "Reviewing",
            "Merging",
            "Complete"
        ]
    );
}

#[test]
fn worker_command_stream_shows_most_recent_first() {
    let entries = vec![
        CommandEntry {
            timestamp: "10:00:00".to_string(),
            command: "first".to_string(),
        },
        CommandEntry {
            timestamp: "10:00:10".to_string(),
            command: "second".to_string(),
        },
        CommandEntry {
            timestamp: "10:00:20".to_string(),
            command: "third".to_string(),
        },
    ];
    assert_eq!(
        worker_command_stream(&entries),
        "10:00:20  third  |  10:00:10  second  |  10:00:00  first"
    );
}

#[test]
fn command_stream_window_truncates_without_scrolling() {
    let long = "long command stream that should be truncated";
    assert_eq!(command_stream_window(long, 10), "long comm…");
}

#[test]
fn worker_flow_chain_normalizes_case_and_whitespace_before_display() {
    let spans = worker_flow_chain_spans("  PLANNING ");
    let labels = spans
        .iter()
        .map(|span| span.content.to_string())
        .filter(|label| label != " → ")
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            "Understand",
            "Planning",
            "Doing",
            "Gitting",
            "Reviewing",
            "Merging",
            "Complete"
        ]
    );
}

#[test]
fn worker_flow_chain_treats_handoff_as_merging_state() {
    let spans = worker_flow_chain_spans("handoff");
    let labels = spans
        .iter()
        .map(|span| span.content.to_string())
        .filter(|label| label != " → ")
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            "Understand",
            "Planning",
            "Doing",
            "Gitting",
            "Reviewing",
            "Merging",
            "Complete"
        ]
    );
    assert_eq!(format_state_label("handoff"), "Merging");
}

#[test]
fn worker_flow_chain_handles_state_prefixes_for_early_states() {
    for state in ["state>planning", "state planning", "\"understand\""] {
        let spans = worker_flow_chain_spans(state);
        let labels = spans
            .iter()
            .map(|span| span.content.to_string())
            .filter(|label| label != " → ")
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec![
                "Understand",
                "Planning",
                "Doing",
                "Gitting",
                "Reviewing",
                "Merging",
                "Complete"
            ],
            "full flow chain not rendered for state '{state}'"
        );
    }
}

#[test]
fn active_worker_displays_current_state_label() {
    let frame = render_dashboard(
        &[WorkerRow {
            worker_id: "w1".to_string(),
            state: "merge_polling".to_string(),
            task_id: Some("task-merge".to_string()),
            last_state_line: 0,
            task_title: "merge worker".to_string(),
            tool_line: "git merge".to_string(),
            breadcrumb: "state>merge_polling".to_string(),
            last_heartbeat_secs: 12,
            session_age_secs: 33,
            lease_held: true,
            session_missing: false,
            command_details: Vec::new(),
        }],
        &QueueStats {
            ready: 0,
            active: 1,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 0,
            p1: 1,
            p2: 0,
        },
        &BacklogView::default(),
        120,
        24,
    );
    assert!(frame.contains("State:"));
    assert!(frame.contains("Checking mergeability"));
}

#[test]
fn triage_mode_renders_activity_and_artifact_cards() {
    let frame = render_triage(
        &["Detecting coding agent signals".to_string()],
        &["repo-intelligence.toml (pending)".to_string()],
        80,
        20,
    );
    assert!(frame.contains("triage mode"));
    assert!(frame.contains("Live Activity"));
    assert!(frame.contains("Triage Artifacts"));
    assert!(frame.contains("Detecting coding agent signals"));
}

#[test]
fn triage_stage_progress_comes_from_activity_stream() {
    let state = AppState::from_triage_feed(
        &[
            "Starting triage session".to_string(),
            "Detecting coding agent signals".to_string(),
            "Interview complete".to_string(),
        ],
        &[],
        super::StartupHeadline {
            spinner_frame: 0,
            verb: "Triage".to_string(),
            startup_active: false,
            ellipsis_phase: 0,
        },
    );
    assert_eq!(state.triage_stages[0].state, StageState::Done);
    assert_eq!(state.triage_stages[1].state, StageState::Done);
    assert_eq!(state.triage_stages[2].state, StageState::Done);
    assert_eq!(state.triage_stages[3].state, StageState::Current);
}

#[test]
fn command_stream_is_rendered_per_worker() {
    let frame = render_dashboard(
        &[
            WorkerRow {
                worker_id: "w1".to_string(),
                state: "doing".to_string(),
                task_id: Some("task-1".to_string()),
                last_state_line: 0,
                task_title: "task one".to_string(),
                tool_line: "tool".to_string(),
                breadcrumb: "understand>doing".to_string(),
                last_heartbeat_secs: 0,
                session_age_secs: 0,
                lease_held: true,
                session_missing: false,
                command_details: vec![("12:34:56".to_string(), "echo first".to_string())],
            },
            WorkerRow {
                worker_id: "w2".to_string(),
                state: "reviewing".to_string(),
                task_id: Some("task-2".to_string()),
                last_state_line: 0,
                task_title: "task two".to_string(),
                tool_line: "tool".to_string(),
                breadcrumb: "reviewing".to_string(),
                last_heartbeat_secs: 0,
                session_age_secs: 0,
                lease_held: true,
                session_missing: false,
                command_details: vec![("23:45:01".to_string(), "echo second".to_string())],
            },
        ],
        &QueueStats {
            ready: 0,
            active: 2,
            failed: 0,
            unresolved: 0,
            merge_pending: 0,
            p0: 0,
            p1: 2,
            p2: 0,
        },
        &BacklogView::default(),
        120,
        24,
    );
    assert!(frame.contains("Flow:"));
    assert!(frame.contains("12:34:56  echo first"));
    assert!(frame.contains("23:45:01  echo second"));
}

#[test]
fn worker_metrics_are_derived_from_states() {
    let workers = vec![
        WorkerCard {
            name: "w1".to_string(),
            state: "doing".to_string(),
            task: String::new(),
            tool_line: String::new(),
            breadcrumb: String::new(),
            activity: Vec::new(),
            command_details: Vec::new(),
            state_bucket: WorkerState::Doing,
            last_heartbeat_secs: 0,
            lease_held: false,
            session_missing: false,
        },
        WorkerCard {
            name: "w2".to_string(),
            state: "reviewing".to_string(),
            task: String::new(),
            tool_line: String::new(),
            breadcrumb: String::new(),
            activity: Vec::new(),
            command_details: Vec::new(),
            state_bucket: WorkerState::Reviewing,
            last_heartbeat_secs: 0,
            lease_held: false,
            session_missing: false,
        },
        WorkerCard {
            name: "w3".to_string(),
            state: "idle".to_string(),
            task: String::new(),
            tool_line: String::new(),
            breadcrumb: String::new(),
            activity: Vec::new(),
            command_details: Vec::new(),
            state_bucket: WorkerState::Idle,
            last_heartbeat_secs: 0,
            lease_held: false,
            session_missing: false,
        },
    ];
    let metrics = WorkerMetrics::from_app_state(&workers);
    assert_eq!(metrics.total, 3);
    assert_eq!(metrics.doing, 1);
    assert_eq!(metrics.reviewing, 1);
    assert_eq!(metrics.idle, 1);
}

#[test]
fn startup_headline_stops_after_30_ticks() {
    let running = StartupHeadlineView::from_tick(29, 0);
    let frozen = StartupHeadlineView::from_tick(35, 0);
    assert!(running.startup_active);
    assert!(!frozen.startup_active);
    assert_eq!(running.spinner(), frozen.spinner());
}

#[test]
fn workers_panel_uses_scrollable_viewport() {
    reset_workers_scroll();
    let workers = (1..=9)
        .map(|idx| WorkerRow {
            worker_id: format!("w{idx}"),
            state: "doing".to_string(),
            task_id: None,
            last_state_line: 0,
            task_title: format!("task {idx}"),
            tool_line: "tool".to_string(),
            breadcrumb: "understand>doing".to_string(),
            last_heartbeat_secs: 0,
            session_age_secs: 0,
            lease_held: true,
            session_missing: false,
            command_details: Vec::new(),
        })
        .collect::<Vec<_>>();
    let stats = QueueStats {
        ready: 0,
        active: workers.len(),
        failed: 0,
        unresolved: 0,
        merge_pending: 0,
        p0: 0,
        p1: workers.len(),
        p2: 0,
    };
    let backlog = BacklogView::default();

    let initial = render_dashboard(&workers, &stats, &backlog, 80, 24);
    assert!(!initial.contains("Workers:"));
    assert!(!initial.contains("Workers ("));
    assert!(initial.contains("> Worker 1"));
    assert!(!initial.contains("Worker 9"));

    for _ in 0..10 {
        let _ = scroll_workers_down();
    }
    let scrolled = render_dashboard(&workers, &stats, &backlog, 80, 24);
    assert!(!scrolled.contains("Worker 1"));
    assert!(!scroll_workers_down());

    for _ in 0..10 {
        let _ = scroll_workers_up();
    }
    let reset = render_dashboard(&workers, &stats, &backlog, 80, 24);
    assert!(reset.contains("> Worker 1"));
    assert!(!scroll_workers_up());
}

#[test]
fn wizard_step_labels_has_five_steps_with_backlog() {
    assert_eq!(super::WIZARD_STEP_LABELS.len(), 5);
    assert_eq!(super::WIZARD_STEP_LABELS[3], "Backlog");
    assert_eq!(
        super::WIZARD_STEP_LABELS,
        ["Parallelism", "Validation", "Docs", "Backlog", "Notes"]
    );
}

#[test]
fn wizard_step_indicator_highlights_backlog_at_step_3() {
    let line = super::wizard_step_indicator(3);
    let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
    assert!(
        text.contains("Backlog"),
        "step indicator should show Backlog"
    );
    assert!(text.contains("Parallelism"));
    assert!(text.contains("Notes"));
}

#[test]
fn wizard_answers_includes_backlog_approval() {
    let answers = super::RepoHealthWizardAnswers {
        preferred_parallelism: 3,
        validation_command: "cargo test".to_string(),
        external_docs_accessible: true,
        backlog_approval: true,
        additional_context: String::new(),
    };
    assert!(answers.backlog_approval);

    let auto = super::RepoHealthWizardAnswers {
        backlog_approval: false,
        ..answers
    };
    assert!(!auto.backlog_approval);
}

#[test]
fn seeding_screen_renders_header_and_activity() {
    let activity = vec![
        "Exploring repository structure".to_string(),
        "Analyzing code quality signals".to_string(),
    ];
    let frame = super::render_seeding(&activity, 80, 20);
    assert!(
        frame.contains("seeding your backlog"),
        "should show seeding header"
    );
    assert!(
        frame.contains("Exploring repository"),
        "should show activity lines"
    );
    assert!(
        frame.contains("Analyzing code quality"),
        "should show all activity"
    );
}

#[test]
fn seeding_screen_renders_empty_activity() {
    let frame = super::render_seeding(&[], 80, 20);
    assert!(frame.contains("seeding your backlog"));
}

#[test]
fn quality_intro_screen_renders_header_and_dimensions() {
    let frame = super::render_quality_intro(120, 20);
    assert!(
        frame.contains("grading your repository"),
        "should show grading header"
    );
    assert!(
        frame.contains("Quality Dimensions"),
        "should show Quality Dimensions block title"
    );
    assert!(
        frame.contains("test_coverage"),
        "should show test_coverage dimension"
    );
    assert!(
        frame.contains("agent_steering"),
        "should show agent_steering dimension"
    );
    assert!(
        frame.contains("documentation_quality"),
        "should show documentation_quality dimension"
    );
    assert!(
        frame.contains("assessing 9 quality dimensions"),
        "should show footer message"
    );
}

#[test]
fn quality_dimensions_has_nine_entries() {
    assert_eq!(
        super::QUALITY_DIMENSIONS.len(),
        9,
        "should have exactly 9 quality dimensions"
    );
}

#[test]
fn quality_dimensions_ids_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for (id, _) in super::QUALITY_DIMENSIONS {
        assert!(seen.insert(id), "duplicate dimension ID: {id}");
    }
}

#[test]
fn style_activity_line_styles_agent_activity_command() {
    let line = "Agent activity: shell started: `cargo test`";
    let styled = super::style_activity_line(line);
    assert_eq!(
        styled.spans.len(),
        3,
        "agent activity line should have 3 spans"
    );
    let command_span = &styled.spans[2];
    assert!(
        command_span.content.contains("cargo test"),
        "command span should contain the command"
    );
}

#[test]
fn style_activity_line_handles_plain_line() {
    let line = "Agent session started";
    let styled = super::style_activity_line(line);
    assert_eq!(styled.spans.len(), 2, "plain line should have 2 spans");
    assert!(
        styled.spans[1].content.contains("Agent session started"),
        "body span should contain the message"
    );
}

#[test]
fn seed_review_renders_task_card_with_all_fields() {
    use crate::seed_runner::SeedTask;
    let task = SeedTask {
        title: "Add AGENTS.md configuration".to_string(),
        details: "Create an AGENTS.md file with repo conventions".to_string(),
        rationale: "Helps agents understand repo norms faster".to_string(),
        domain: "agent_steering".to_string(),
        priority: "P0".to_string(),
    };
    let frame = super::render_seed_review(&task, 0, 5, 100, 25);
    assert!(
        frame.contains("review backlog"),
        "header should show review backlog"
    );
    assert!(frame.contains("(1/5)"), "should show 1-indexed counter");
    assert!(frame.contains("Add AGENTS.md"), "should show task title");
    assert!(
        frame.contains("Helps agents understand"),
        "should show rationale"
    );
    assert!(frame.contains("P0"), "should show priority badge");
    assert!(frame.contains("[k] Keep"), "should show keep hotkey");
    assert!(frame.contains("[d] Discard"), "should show discard hotkey");
    assert!(frame.contains("[q]"), "should show quit hotkey");
}

#[test]
fn seed_review_renders_different_priorities() {
    use crate::seed_runner::SeedTask;
    for priority in &["P0", "P1", "P2"] {
        let task = SeedTask {
            title: "Task".to_string(),
            details: "Details".to_string(),
            rationale: "Rationale".to_string(),
            domain: "testing".to_string(),
            priority: priority.to_string(),
        };
        let frame = super::render_seed_review(&task, 2, 10, 80, 20);
        assert!(frame.contains("(3/10)"), "counter should be 1-indexed");
        assert!(frame.contains(priority), "should show {priority}");
    }
}

use super::{WizardAction, WizardState};
use crossterm::event::{KeyCode, KeyModifiers};

fn wizard_at_step(step: usize) -> WizardState {
    WizardState {
        step,
        parallelism_input: "3".to_string(),
        validation: "cargo test".to_string(),
        docs_accessible: true,
        backlog_approval: true,
        notes: String::new(),
    }
}

#[test]
fn wizard_backlog_a_key_selects_auto_seed_and_advances() {
    let mut ws = wizard_at_step(3);
    assert!(ws.backlog_approval);
    ws.handle_key(KeyCode::Char('a'), KeyModifiers::NONE);
    assert!(!ws.backlog_approval, "'a' should select auto-seed");
    assert_eq!(ws.step, 4, "'a' should advance to Notes");
}

#[test]
fn wizard_backlog_r_key_selects_review_and_advances() {
    let mut ws = wizard_at_step(3);
    ws.backlog_approval = false;
    ws.handle_key(KeyCode::Char('r'), KeyModifiers::NONE);
    assert!(ws.backlog_approval, "'r' should select review");
    assert_eq!(ws.step, 4, "'r' should advance to Notes");
}

#[test]
fn wizard_backlog_uppercase_keys_select_and_advance() {
    let mut ws = wizard_at_step(3);
    ws.handle_key(KeyCode::Char('A'), KeyModifiers::NONE);
    assert!(!ws.backlog_approval, "'A' should select auto-seed");
    assert_eq!(ws.step, 4, "'A' should advance to Notes");
}

#[test]
fn wizard_backlog_arrow_keys_toggle() {
    let mut ws = wizard_at_step(3);
    assert!(ws.backlog_approval);
    ws.handle_key(KeyCode::Down, KeyModifiers::NONE);
    assert!(!ws.backlog_approval, "Down should toggle to auto-seed");
    ws.handle_key(KeyCode::Up, KeyModifiers::NONE);
    assert!(ws.backlog_approval, "Up should toggle back to review");
}

#[test]
fn wizard_backlog_tab_toggles() {
    let mut ws = wizard_at_step(3);
    assert!(ws.backlog_approval);
    ws.handle_key(KeyCode::Tab, KeyModifiers::NONE);
    assert!(!ws.backlog_approval, "Tab should toggle to auto-seed");
    ws.handle_key(KeyCode::Tab, KeyModifiers::NONE);
    assert!(ws.backlog_approval, "Tab should toggle back to review");
}

#[test]
fn wizard_backlog_enter_advances_to_notes() {
    let mut ws = wizard_at_step(3);
    let action = ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(action, WizardAction::Continue);
    assert_eq!(ws.step, 4, "Enter should advance to step 4 (Notes)");
}

#[test]
fn wizard_backlog_tab_then_enter_preserves_selection() {
    let mut ws = wizard_at_step(3);
    ws.handle_key(KeyCode::Tab, KeyModifiers::NONE);
    assert!(!ws.backlog_approval);
    assert_eq!(ws.step, 3, "Tab should not advance");
    ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ws.step, 4);
    assert!(
        !ws.backlog_approval,
        "auto-seed selection should persist after Enter"
    );
}

#[test]
fn wizard_esc_finishes_at_any_step() {
    for step in 0..5 {
        let mut ws = wizard_at_step(step);
        let action = ws.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            action,
            WizardAction::Finish,
            "Esc at step {step} should finish"
        );
    }
}

#[test]
fn wizard_step_progression_through_all_steps() {
    let mut ws = wizard_at_step(0);
    ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ws.step, 1);
    ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ws.step, 2);
    ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ws.step, 3);
    ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ws.step, 4);
    let action = ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(action, WizardAction::Finish, "Enter on Notes should finish");
}

#[test]
fn wizard_unrelated_keys_on_backlog_step_ignored() {
    let mut ws = wizard_at_step(3);
    ws.handle_key(KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(
        ws.backlog_approval,
        "unrelated key should not change selection"
    );
    assert_eq!(ws.step, 3, "unrelated key should not change step");
}
