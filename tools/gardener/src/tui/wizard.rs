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

use super::formatting::wizard_step_indicator;
use super::terminal::teardown_terminal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoHealthWizardAnswers {
    pub preferred_parallelism: u32,
    pub validation_command: String,
    pub external_docs_accessible: bool,
    pub backlog_approval: bool,
    pub additional_context: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WizardAction {
    Continue,
    Finish,
}

#[derive(Debug, Clone)]
pub(crate) struct WizardState {
    pub(crate) step: usize,
    pub(crate) parallelism_input: String,
    pub(crate) validation: String,
    pub(crate) docs_accessible: bool,
    pub(crate) backlog_approval: bool,
    pub(crate) notes: String,
}

impl WizardState {
    pub(crate) fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> WizardAction {
        if code == KeyCode::Esc {
            return WizardAction::Finish;
        }
        match self.step {
            0 => match code {
                KeyCode::Enter => self.step = 1,
                KeyCode::Backspace => {
                    self.parallelism_input.pop();
                }
                KeyCode::Char(c)
                    if !modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_digit() =>
                {
                    self.parallelism_input.push(c)
                }
                _ => {}
            },
            1 => match code {
                KeyCode::Enter => self.step = 2,
                KeyCode::Backspace => {
                    self.validation.pop();
                }
                KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    self.validation.push(c)
                }
                _ => {}
            },
            2 => match code {
                KeyCode::Enter => self.step = 3,
                KeyCode::Char('y') | KeyCode::Char('Y') => self.docs_accessible = true,
                KeyCode::Char('n') | KeyCode::Char('N') => self.docs_accessible = false,
                _ => {}
            },
            3 => match code {
                KeyCode::Enter => self.step = 4,
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.backlog_approval = false;
                    self.step = 4;
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.backlog_approval = true;
                    self.step = 4;
                }
                KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                    self.backlog_approval = !self.backlog_approval
                }
                _ => {}
            },
            _ => match code {
                KeyCode::Enter => return WizardAction::Finish,
                KeyCode::Backspace => {
                    self.notes.pop();
                }
                KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    self.notes.push(c)
                }
                _ => {}
            },
        }
        WizardAction::Continue
    }
}

pub fn run_repo_health_wizard(
    default_validation_command: &str,
) -> Result<RepoHealthWizardAnswers, GardenerError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(stdout, EnterAlternateScreen).map_err(|e| GardenerError::Io(e.to_string()))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| GardenerError::Io(e.to_string()))?;

    let mut ws = WizardState {
        step: 0,
        parallelism_input: "3".to_string(),
        validation: default_validation_command.to_string(),
        docs_accessible: true,
        backlog_approval: true,
        notes: String::new(),
    };

    loop {
        let step = ws.step;
        let parallelism_input = &ws.parallelism_input;
        let validation = &ws.validation;
        let docs_accessible = ws.docs_accessible;
        let backlog_approval = ws.backlog_approval;
        let notes = &ws.notes;
        terminal
            .draw(|frame| {
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

                frame.render_widget(Paragraph::new(wizard_step_indicator(step)), chunks[1]);

                let (label, help, value) = match step {
                    0 => (
                        "Worker parallelism",
                        "How many parallel workers should gardener run? Range: 1-32.",
                        format!("> {}█", parallelism_input),
                    ),
                    1 => (
                        "Validation command",
                        "Command to verify code changes. Edit or keep the default.",
                        format!("> {}█", validation),
                    ),
                    2 => (
                        "Architecture docs available?",
                        "Are architecture/quality docs accessible in the repo? Press y/n.",
                        format!("> {}", if docs_accessible { "yes" } else { "no" }),
                    ),
                    3 => (
                        "Backlog seeding",
                        "Gardener seeds a backlog of tasks that make your repo more hospitable to coding agents.",
                        String::new(),
                    ),
                    _ => (
                        "Additional constraints (optional)",
                        "Any extra context for workers? Leave empty to skip.",
                        format!("> {}█", notes),
                    ),
                };
                let selected_style = Style::default()
                    .fg(Color::Rgb(85, 198, 255))
                    .add_modifier(Modifier::BOLD);
                let unselected_style = Style::default().fg(Color::DarkGray);
                let mut body_lines = vec![
                    Line::from(Span::styled(
                        label,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        help,
                        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                    )),
                    Line::from(""),
                ];
                if step == 3 {
                    let (review_dot, review_style) = if backlog_approval {
                        ("● ", selected_style)
                    } else {
                        ("○ ", unselected_style)
                    };
                    let (auto_dot, auto_style) = if !backlog_approval {
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
                        format!("Step {} of 5", step + 1),
                        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        if step < 4 { "Enter →" } else { "Enter to finish" },
                        Style::default().fg(Color::Rgb(170, 178, 210)),
                    ),
                ]))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
                );
                frame.render_widget(footer, chunks[3]);
            })
            .map_err(|e| GardenerError::Io(e.to_string()))?;

        if let Event::Key(key) = event::read().map_err(|e| GardenerError::Io(e.to_string()))? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if ws.handle_key(key.code, key.modifiers) == WizardAction::Finish {
                break;
            }
        }
    }

    teardown_terminal(terminal)?;

    let preferred_parallelism = ws
        .parallelism_input
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0 && *value <= 32)
        .unwrap_or(3);

    Ok(RepoHealthWizardAnswers {
        preferred_parallelism,
        validation_command: if ws.validation.trim().is_empty() {
            default_validation_command.to_string()
        } else {
            ws.validation
        },
        external_docs_accessible: ws.docs_accessible,
        backlog_approval: ws.backlog_approval,
        additional_context: ws.notes,
    })
}
