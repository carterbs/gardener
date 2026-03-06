use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, Paragraph};

use crate::tui::formatting::style_activity_line;

pub fn render_seeding(activity: &[String], width: u16, height: u16) -> String {
    crate::tui::render_to_string(width, height, |frame| draw_seeding_frame(frame, activity))
}

#[cfg(test)]
pub(crate) fn render_shutdown_screen(title: &str, message: &str, width: u16, height: u16) -> String {
    crate::tui::render_to_string(width, height, |frame| {
        draw_shutdown_frame(frame, title, message)
    })
}

pub(crate) fn draw_seeding_frame(frame: &mut ratatui::Frame<'_>, activity: &[String]) {
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
            "seeding your backlog",
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
        vec![ratatui::widgets::ListItem::new(
            "- waiting for seeding updates",
        )]
    } else {
        activity
            .iter()
            .map(|line| ratatui::widgets::ListItem::new(style_activity_line(line)))
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        List::new(activity_items).block(
            Block::default()
                .title("Live Activity")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[1],
    );

    let footer = Paragraph::new("Seeding in progress - agent is exploring your repository").block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(footer, chunks[2]);
}

pub(crate) fn draw_shutdown_frame(frame: &mut ratatui::Frame<'_>, title: &str, message: &str) {
    let is_error =
        title.to_ascii_lowercase().contains("error") || title.to_ascii_lowercase().contains("fail");
    let accent = if is_error {
        Color::Red
    } else {
        Color::Rgb(85, 198, 255)
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "GARDENER ",
                Style::default()
                    .fg(Color::Rgb(85, 198, 255))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                title,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[0],
    );

    let body_lines: Vec<Line> = message
        .lines()
        .map(|line| {
            if line.is_empty() {
                Line::from("")
            } else if line.starts_with("Tasks merged") || line.starts_with("Total runtime") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Rgb(180, 180, 180)),
                ))
            } else if line.starts_with("Tasks failed") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Rgb(255, 150, 100)),
                ))
            } else if line.starts_with("Tasks completed") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(line.to_string())
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(body_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(accent)),
        ),
        chunks[1],
    );

    frame.render_widget(
        Paragraph::new(if is_error {
            "Press Ctrl+C or c to copy the error message, then any key to exit"
        } else {
            "Press any key to exit"
        })
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[2],
    );
}
