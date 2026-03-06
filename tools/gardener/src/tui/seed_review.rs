use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use crate::errors::GardenerError;
use crate::seed_runner::SeedTask;

use super::render_to_string;
use super::terminal::teardown_terminal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    Keep,
    Discard(Option<String>),
    Refine(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    DiscardReason,
    RefineFeedback,
}

pub fn render_seed_review(
    task: &SeedTask,
    index: usize,
    total: usize,
    width: u16,
    height: u16,
) -> String {
    render_to_string(width, height, |frame| {
        draw_seed_review_frame(frame, task, index, total, 0, None, "")
    })
}

fn draw_seed_review_frame(
    frame: &mut ratatui::Frame<'_>,
    task: &SeedTask,
    index: usize,
    total: usize,
    round: usize,
    input_mode: Option<&InputMode>,
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

    let counter = if round == 0 {
        format!("review backlog  ({}/{})", index + 1, total)
    } else {
        format!(
            "review backlog  round {}  ({}/{})",
            round + 1,
            index + 1,
            total
        )
    };
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

    let priority_style = match task.priority.as_str() {
        "P0" => Style::default()
            .fg(Color::Rgb(255, 122, 122))
            .add_modifier(Modifier::BOLD),
        "P1" => Style::default()
            .fg(Color::Rgb(255, 207, 105))
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(Color::Rgb(127, 230, 148))
            .add_modifier(Modifier::BOLD),
    };

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
        let (label, hint) = match mode {
            InputMode::DiscardReason => (
                "Why discard? (optional — Enter to skip, Esc to cancel)",
                Color::Rgb(255, 122, 122),
            ),
            InputMode::RefineFeedback => (
                "How should this task change? (Enter to submit, Esc to cancel)",
                Color::Rgb(245, 196, 95),
            ),
        };
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

pub fn run_seed_review_wizard(
    tasks: &[SeedTask],
    round: usize,
) -> Result<Vec<ReviewDecision>, GardenerError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(stdout, EnterAlternateScreen).map_err(|e| GardenerError::Io(e.to_string()))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| GardenerError::Io(e.to_string()))?;

    let total = tasks.len();
    let mut decisions: Vec<Option<ReviewDecision>> = vec![None; total];
    let mut current = 0usize;
    let mut input_mode: Option<InputMode> = None;
    let mut input_buffer = String::new();

    loop {
        if current >= total {
            break;
        }
        terminal
            .draw(|frame| {
                draw_seed_review_frame(
                    frame,
                    &tasks[current],
                    current,
                    total,
                    round,
                    input_mode.as_ref(),
                    &input_buffer,
                )
            })
            .map_err(|e| GardenerError::Io(e.to_string()))?;

        if let Event::Key(key) = event::read().map_err(|e| GardenerError::Io(e.to_string()))? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if let Some(mode) = input_mode {
                match key.code {
                    KeyCode::Enter => match mode {
                        InputMode::DiscardReason => {
                            let reason = if input_buffer.trim().is_empty() {
                                None
                            } else {
                                Some(input_buffer.clone())
                            };
                            decisions[current] = Some(ReviewDecision::Discard(reason));
                            current += 1;
                            input_mode = None;
                            input_buffer.clear();
                        }
                        InputMode::RefineFeedback => {
                            if !input_buffer.trim().is_empty() {
                                decisions[current] =
                                    Some(ReviewDecision::Refine(input_buffer.clone()));
                                current += 1;
                                input_mode = None;
                                input_buffer.clear();
                            }
                        }
                    },
                    KeyCode::Esc => {
                        input_mode = None;
                        input_buffer.clear();
                    }
                    KeyCode::Backspace => {
                        input_buffer.pop();
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        input_buffer.push(c);
                    }
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('k') | KeyCode::Char('K') => {
                        decisions[current] = Some(ReviewDecision::Keep);
                        current += 1;
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') => {
                        input_mode = Some(InputMode::DiscardReason);
                        input_buffer.clear();
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        input_mode = Some(InputMode::RefineFeedback);
                        input_buffer.clear();
                    }
                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => break,
                    _ => {}
                }
            }
        }
    }

    teardown_terminal(terminal)?;

    Ok(decisions
        .into_iter()
        .map(|d| d.unwrap_or(ReviewDecision::Discard(None)))
        .collect())
}
