use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use serde_json::json;

use crate::errors::GardenerError;
use crate::logging::append_run_log;

use super::formatting::style_activity_line;
use super::render_to_string;
use super::terminal::with_live_terminal;

#[cfg(test)]
thread_local! {
    static TEST_LIVE_QUALITY_BYPASS: std::cell::RefCell<bool> = const { std::cell::RefCell::new(false) };
}

#[cfg(test)]
fn with_live_quality_bypass<T>(f: impl FnOnce() -> T) -> T {
    TEST_LIVE_QUALITY_BYPASS.with(|cell| {
        let previous = *cell.borrow();
        *cell.borrow_mut() = true;
        let result = f();
        *cell.borrow_mut() = previous;
        result
    })
}

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

fn draw_quality_live<F>(draw: F) -> Result<(), GardenerError>
where
    F: FnOnce(&mut ratatui::Frame<'_>),
{
    append_run_log("debug", "tui.quality.draw_live_frame", json!({}));
    #[cfg(test)]
    if TEST_LIVE_QUALITY_BYPASS.with(|cell| *cell.borrow()) {
        let _ = render_to_string(80, 18, draw);
        return Ok(());
    }
    with_live_terminal(|terminal| {
        terminal
            .draw(draw)
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

pub fn draw_quality_grading_live(activity: &[String]) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "tui.quality.draw_grading_live",
        json!({ "activity_lines": activity.len() }),
    );
    draw_quality_live(|frame| draw_quality_grading_frame(frame, activity))
}

pub fn render_quality_grading(activity: &[String], width: u16, height: u16) -> String {
    render_to_string(width, height, |frame| {
        draw_quality_grading_frame(frame, activity)
    })
}

fn quality_activity_lines(activity: &[String]) -> Vec<String> {
    if activity.is_empty() {
        vec!["- waiting for quality grading updates".to_string()]
    } else {
        activity.to_vec()
    }
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

    let activity_items = quality_activity_lines(activity)
        .iter()
        .map(|line| ListItem::new(style_activity_line(line)))
        .collect::<Vec<_>>();
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
    append_run_log("debug", "tui.quality.draw_intro_live", json!({}));
    draw_quality_live(draw_quality_intro_frame)
}

pub fn render_quality_intro(width: u16, height: u16) -> String {
    render_to_string(width, height, draw_quality_intro_frame)
}

fn quality_dimension_lines() -> Vec<String> {
    QUALITY_DIMENSIONS
        .iter()
        .map(|(name, desc)| format!("  {name}  —  {desc}"))
        .collect()
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

    let dimension_lines = quality_dimension_lines();
    let dimension_items: Vec<ListItem<'_>> = dimension_lines
        .iter()
        .zip(QUALITY_DIMENSIONS.iter())
        .map(|(line, (name, desc))| {
            ListItem::new(Line::from(vec![
                Span::raw(&line[..2]),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_quality_intro_lists_all_dimension_ids() {
        let frame = render_quality_intro(140, 24);
        for (name, _) in QUALITY_DIMENSIONS {
            assert!(frame.contains(name), "missing dimension {name}");
        }
        assert!(frame.contains("Quality Dimensions"));
        assert!(frame.contains("assessing 9 quality dimensions"));
    }

    #[test]
    fn render_quality_grading_shows_activity_lines_and_footer() {
        let activity = vec![
            "Scanning repository evidence".to_string(),
            "Comparing coverage infrastructure".to_string(),
            "Scoring mechanical guardrails".to_string(),
        ];
        let frame = render_quality_grading(&activity, 110, 20);
        assert!(frame.contains("Quality Grading Activity"));
        assert!(frame.contains("Scanning repository evidence"));
        assert!(frame.contains("Comparing coverage infrastructure"));
        assert!(frame.contains("Scoring mechanical guardrails"));
        assert!(frame.contains("Quality grading in progress"));
    }

    #[test]
    fn render_quality_grading_uses_waiting_state_when_empty() {
        let frame = render_quality_grading(&[], 90, 16);
        assert!(frame.contains("grading your repository"));
        assert!(frame.contains("waiting for quality grading updates"));
    }

    #[test]
    fn quality_activity_lines_preserve_order_and_fallback_copy() {
        assert_eq!(
            quality_activity_lines(&[]),
            vec!["- waiting for quality grading updates".to_string()]
        );
        assert_eq!(
            quality_activity_lines(&[
                "first".to_string(),
                "second".to_string(),
                "third".to_string()
            ]),
            vec![
                "first".to_string(),
                "second".to_string(),
                "third".to_string()
            ]
        );
    }

    #[test]
    fn quality_dimension_lines_cover_every_dimension_description() {
        let lines = quality_dimension_lines();
        assert_eq!(lines.len(), QUALITY_DIMENSIONS.len());
        for ((name, desc), line) in QUALITY_DIMENSIONS.iter().zip(lines.iter()) {
            assert!(line.contains(name));
            assert!(line.contains(desc));
        }
    }

    #[test]
    fn live_quality_draw_wrappers_execute_under_test_bypass() {
        with_live_quality_bypass(|| {
            draw_quality_grading_live(&["Scanning coverage".to_string()]).expect("grading draw");
            draw_quality_intro_live().expect("intro draw");
        });
    }
}
