use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::errors::GardenerError;

use super::formatting::style_activity_line;
use super::render_to_string;
use super::terminal::with_live_terminal;

/// Canonical quality dimension descriptions shown in the intro screen.
/// The linter in `tests/quality_dimension_linter.rs` enforces that these IDs
/// match the dimension keys used in `quality_assessment_runner.rs`.
pub const QUALITY_DIMENSIONS: &[(&str, &str)] = &[
    (
        "test_coverage",
        "Measures what fraction of source files have corresponding tests, weighting integration and e2e tests more heavily.",
    ),
    (
        "test_quality",
        "Evaluates how thorough tests are: assertion density, edge-case coverage, test isolation, and meaningful naming.",
    ),
    (
        "risk_exposure",
        "Assesses how exposed each domain is to bugs and regressions based on complexity metrics and untested code.",
    ),
    (
        "convention_adherence",
        "Checks how consistently the codebase follows its own stated conventions for style, naming, and linting.",
    ),
    (
        "agent_steering",
        "Rates the quality of AGENTS.md/CLAUDE.md steering docs: specificity, architecture pointers, and build commands.",
    ),
    (
        "mechanical_guardrails",
        "Evaluates breadth and enforcement of automated checks: linters, formatters, type checkers, and CI gates.",
    ),
    (
        "local_feedback_loop",
        "Measures how quickly developers can validate changes locally via test runners, Makefiles, and watch modes.",
    ),
    (
        "coverage_infrastructure",
        "Checks whether code coverage is measured, reported, and enforced with thresholds and CI gates.",
    ),
    (
        "documentation_quality",
        "Assesses README quality, API docs, architectural docs, inline doc density, and doc generation setup.",
    ),
];

pub fn draw_quality_grading_live(activity: &[String]) -> Result<(), GardenerError> {
    with_live_terminal(|terminal| {
        terminal
            .draw(|frame| draw_quality_grading_frame(frame, activity))
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

pub fn render_quality_grading(activity: &[String], width: u16, height: u16) -> String {
    render_to_string(width, height, |frame| {
        draw_quality_grading_frame(frame, activity)
    })
}

fn draw_quality_grading_frame(frame: &mut ratatui::Frame<'_>, activity: &[String]) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "grading your repository",
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
    frame.render_widget(header, chunks[0]);

    let activity_items = if activity.is_empty() {
        vec![ListItem::new("- waiting for quality grading updates")]
    } else {
        activity
            .iter()
            .map(|line| ListItem::new(style_activity_line(line)))
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        List::new(activity_items).block(
            Block::default()
                .title("Quality Grading Activity")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[1],
    );

    let footer =
        Paragraph::new("Quality grading in progress — agents are assessing your repository").block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        );
    frame.render_widget(footer, chunks[2]);
}

pub fn draw_quality_intro_live() -> Result<(), GardenerError> {
    with_live_terminal(|terminal| {
        terminal
            .draw(draw_quality_intro_frame)
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

pub fn render_quality_intro(width: u16, height: u16) -> String {
    render_to_string(width, height, draw_quality_intro_frame)
}

fn draw_quality_intro_frame(frame: &mut ratatui::Frame<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "grading your repository",
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
    frame.render_widget(header, chunks[0]);

    let dimension_items: Vec<ListItem<'_>> = QUALITY_DIMENSIONS
        .iter()
        .map(|(name, desc)| {
            ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    *name,
                    Style::default()
                        .fg(Color::Rgb(85, 198, 255))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  —  ", Style::default().fg(Color::Gray)),
                Span::styled(*desc, Style::default().fg(Color::Gray)),
            ]))
        })
        .collect();

    frame.render_widget(
        List::new(dimension_items).block(
            Block::default()
                .title("Quality Dimensions")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[1],
    );

    let footer = Paragraph::new("Agents are starting up — assessing 9 quality dimensions").block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(footer, chunks[2]);
}
