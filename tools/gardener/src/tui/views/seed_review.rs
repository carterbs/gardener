use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::seed_runner::SeedTask;
use crate::tui::state::seed_review::InputMode;

pub(crate) fn draw_seed_review_frame(
    frame: &mut ratatui::Frame<'_>,
    task: &SeedTask,
    index: usize,
    total: usize,
    round: usize,
    input_mode: Option<InputMode>,
    input_text: &str,
) {
    let footer_height = if input_mode.is_some() { 4 } else { 2 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(footer_height),
        ])
        .split(frame.area());

    let counter = review_counter(index, total, round);
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            counter,
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

    let priority_style = priority_style(&task.priority);

    let body_lines = vec![
        Line::from(Span::styled(
            task.title.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            task.details.clone(),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Why this helps agents: ",
                Style::default()
                    .fg(Color::Rgb(85, 198, 255))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                task.rationale.clone(),
                Style::default().fg(Color::Rgb(85, 198, 255)),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Priority: ", Style::default().fg(Color::Gray)),
            Span::styled(task.priority.clone(), priority_style),
            Span::styled(
                format!("    Domain: {}", task.domain),
                Style::default().fg(Color::Gray),
            ),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(body_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[1],
    );

    if let Some(mode) = input_mode {
        let (label, hint) = input_mode_prompt(mode);
        let footer = Paragraph::new(vec![
            Line::from(Span::styled(
                label,
                Style::default().fg(hint).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("> {input_text}█"),
                Style::default().fg(Color::White),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        );
        frame.render_widget(footer, chunks[2]);
    } else {
        let footer = Paragraph::new(Line::from(vec![
            Span::styled(
                "[k] Keep",
                Style::default()
                    .fg(Color::Rgb(126, 231, 135))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    "),
            Span::styled(
                "[d] Discard",
                Style::default()
                    .fg(Color::Rgb(255, 122, 122))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    "),
            Span::styled(
                "[r] Refine",
                Style::default()
                    .fg(Color::Rgb(245, 196, 95))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    "),
            Span::styled(
                "[q] Discard remaining & finish",
                Style::default().fg(Color::Gray),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        );
        frame.render_widget(footer, chunks[2]);
    }
}

pub(crate) fn review_counter(index: usize, total: usize, round: usize) -> String {
    if round == 0 {
        format!("review backlog  ({}/{})", index + 1, total)
    } else {
        format!(
            "review backlog  round {}  ({}/{})",
            round + 1,
            index + 1,
            total
        )
    }
}

fn priority_style(priority: &str) -> Style {
    match priority {
        "P0" => Style::default()
            .fg(Color::Rgb(255, 122, 122))
            .add_modifier(Modifier::BOLD),
        "P1" => Style::default()
            .fg(Color::Rgb(255, 207, 105))
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(Color::Rgb(127, 230, 148))
            .add_modifier(Modifier::BOLD),
    }
}

pub(crate) fn input_mode_prompt(mode: InputMode) -> (&'static str, Color) {
    match mode {
        InputMode::DiscardReason => (
            "Why discard? (optional — Enter to skip, Esc to cancel)",
            Color::Rgb(255, 122, 122),
        ),
        InputMode::RefineFeedback => (
            "How should this task change? (Enter to submit, Esc to cancel)",
            Color::Rgb(245, 196, 95),
        ),
    }
}

pub fn render_seed_review(
    task: &SeedTask,
    index: usize,
    total: usize,
    width: u16,
    height: u16,
) -> String {
    crate::tui::render_to_string(width, height, |frame| {
        draw_seed_review_frame(frame, task, index, total, 0, None, "")
    })
}

#[cfg(test)]
pub(crate) fn render_seed_review_discard_prompt(
    task: &SeedTask,
    index: usize,
    total: usize,
    round: usize,
    input_text: &str,
    width: u16,
    height: u16,
) -> String {
    crate::tui::render_to_string(width, height, |frame| {
        draw_seed_review_frame(
            frame,
            task,
            index,
            total,
            round,
            Some(InputMode::DiscardReason),
            input_text,
        )
    })
}

#[cfg(test)]
pub(crate) fn render_seed_review_refine_prompt(
    task: &SeedTask,
    index: usize,
    total: usize,
    round: usize,
    input_text: &str,
    width: u16,
    height: u16,
) -> String {
    crate::tui::render_to_string(width, height, |frame| {
        draw_seed_review_frame(
            frame,
            task,
            index,
            total,
            round,
            Some(InputMode::RefineFeedback),
            input_text,
        )
    })
}
