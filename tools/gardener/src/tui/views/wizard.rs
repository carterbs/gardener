use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::formatting::wizard_step_indicator;
use crate::tui::state::wizard::WizardState;

pub(crate) fn draw_wizard_frame(frame: &mut ratatui::Frame<'_>, ws: &WizardState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "setup wizard",
            Style::default()
                .fg(Color::Rgb(245, 196, 95))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("    "),
        Span::styled(
            "Esc = keep defaults",
            Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(header, chunks[0]);

    frame.render_widget(Paragraph::new(wizard_step_indicator(ws.step)), chunks[1]);

    let (label, help, value) = match ws.step {
        0 => (
            "Worker parallelism",
            "How many parallel workers should gardener run? Range: 1-32.",
            format!("> {}█", ws.parallelism_input),
        ),
        1 => (
            "Validation command",
            "Command to verify code changes. Edit or keep the default.",
            format!("> {}█", ws.validation),
        ),
        2 => (
            "Architecture docs available?",
            "Are architecture/quality docs accessible in the repo? Press y/n.",
            format!("> {}", if ws.docs_accessible { "yes" } else { "no" }),
        ),
        3 => (
            "Backlog seeding",
            "Gardener seeds a backlog of tasks that make your repo more hospitable to coding agents.",
            String::new(),
        ),
        _ => (
            "Additional constraints (optional)",
            "Any extra context for workers? Leave empty to skip.",
            format!("> {}█", ws.notes),
        ),
    };
    let selected_style = Style::default()
        .fg(Color::Rgb(85, 198, 255))
        .add_modifier(Modifier::BOLD);
    let unselected_style = Style::default().fg(Color::DarkGray);
    let mut body_lines = vec![
        Line::from(Span::styled(
            label,
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            help,
            Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
        )),
        Line::from(""),
    ];
    if ws.step == 3 {
        let (review_dot, review_style) = if ws.backlog_approval {
            ("● ", selected_style)
        } else {
            ("○ ", unselected_style)
        };
        let (auto_dot, auto_style) = if !ws.backlog_approval {
            ("● ", selected_style)
        } else {
            ("○ ", unselected_style)
        };
        body_lines.push(Line::from(vec![
            Span::styled(review_dot, review_style),
            Span::styled("review tasks", review_style),
            Span::styled("  (r)", Style::default().fg(Color::DarkGray)),
        ]));
        body_lines.push(Line::from(""));
        body_lines.push(Line::from(vec![
            Span::styled(auto_dot, auto_style),
            Span::styled("auto-seed", auto_style),
            Span::styled("  (a)", Style::default().fg(Color::DarkGray)),
        ]));
    } else {
        body_lines.push(Line::from(Span::styled(
            value,
            Style::default().fg(Color::Rgb(85, 198, 255)),
        )));
    }
    let body = Paragraph::new(body_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(body, chunks[2]);

    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            format!("Step {} of 5", ws.step + 1),
            Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
        ),
        Span::raw("  "),
        Span::styled(
            if ws.step < 4 { "Enter →" } else { "Enter to finish" },
            Style::default().fg(Color::Rgb(170, 178, 210)),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(footer, chunks[3]);
}

#[cfg(test)]
pub(crate) fn render_wizard_state(ws: &WizardState, width: u16, height: u16) -> String {
    crate::tui::render_to_string(width, height, |frame| draw_wizard_frame(frame, ws))
}
