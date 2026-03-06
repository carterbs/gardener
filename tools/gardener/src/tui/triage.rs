use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use super::render_to_string;
use super::state::{AppState, StageState, StartupHeadline, TriageStage};

const TRIAGE_STAGE_LABELS: [&str; 4] = [
    "Scan repository shape",
    "Detect tools and docs",
    "Build project profile",
    "Seed prioritized backlog",
];

pub fn render_triage(activity: &[String], artifacts: &[String], width: u16, height: u16) -> String {
    render_to_string(width, height, |frame| {
        draw_triage_frame(frame, activity, artifacts)
    })
}

pub(super) fn triage_stage_progress(activity: &[String]) -> usize {
    let mut current_stage = 0usize;
    for entry in activity {
        let lower = entry.to_ascii_lowercase();
        if lower.contains("persisted triage profile") || lower.contains("interview complete") {
            current_stage = 3;
        } else if lower.contains("discovery assessment complete")
            || lower.contains("running repository discovery assessment")
        {
            current_stage = 2;
        } else if lower.contains("collecting human-validated repository context") {
            current_stage = current_stage.max(2);
        } else if lower.contains("agent detection complete")
            || lower.contains("detecting coding agent signals")
        {
            current_stage = current_stage.max(1);
        } else if lower.contains("starting triage session") {
            current_stage = current_stage.max(0);
        }
    }
    current_stage
}

pub(super) fn triage_stages_with_state(current_stage: usize) -> Vec<TriageStage> {
    TRIAGE_STAGE_LABELS
        .iter()
        .enumerate()
        .map(|(index, label)| TriageStage {
            label: (*label).to_string(),
            state: if index < current_stage {
                StageState::Done
            } else if index == current_stage {
                StageState::Current
            } else {
                StageState::Future
            },
        })
        .collect()
}

pub(super) fn draw_triage_frame(
    frame: &mut ratatui::Frame<'_>,
    activity: &[String],
    artifacts: &[String],
) {
    let mut app_state = AppState::from_triage_feed(
        activity,
        artifacts,
        StartupHeadline {
            spinner_frame: 0,
            verb: "Triage".to_string(),
            startup_active: false,
            ellipsis_phase: 0,
        },
    );
    let viewport = frame.area();
    app_state.terminal_width = viewport.width;
    app_state.terminal_height = viewport.height;
    draw_triage_frame_from_state(frame, &app_state);
}

fn draw_triage_frame_from_state(frame: &mut ratatui::Frame<'_>, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());
    let triage_narrow = state.terminal_width < 80;
    let body = Layout::default()
        .direction(if triage_narrow {
            Direction::Vertical
        } else {
            Direction::Horizontal
        })
        .constraints(if triage_narrow {
            [Constraint::Min(1), Constraint::Min(1)]
        } else {
            [Constraint::Percentage(62), Constraint::Percentage(38)]
        })
        .split(chunks[1]);

    let summary = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "triage mode",
            Style::default()
                .fg(Color::Rgb(245, 196, 95))
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(summary, chunks[0]);

    let activity_items = if state.triage_activity.is_empty() {
        vec![ListItem::new("- waiting for triage updates")]
    } else {
        state
            .triage_activity
            .iter()
            .map(|entry| ListItem::new(format!("- {} {}", entry.timestamp, entry.message)))
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        List::new(activity_items).block(
            Block::default()
                .title("Live Activity")
                .borders(Borders::RIGHT),
        ),
        body[0],
    );

    let artifact_items = if state.triage_artifacts.is_empty() {
        vec![ListItem::new("- no triage artifacts yet")]
    } else {
        state
            .triage_artifacts
            .iter()
            .map(|entry| {
                if entry.value.is_empty() {
                    ListItem::new(format!("- {}", entry.label))
                } else {
                    ListItem::new(format!("- {}: {}", entry.label, entry.value))
                }
            })
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        List::new(artifact_items).block(Block::default().title("Triage Artifacts")),
        body[1],
    );

    let footer = Paragraph::new("Controls: triage in progress; follow prompts in terminal").block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(footer, chunks[2]);
}
