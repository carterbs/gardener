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

fn finalize_answers(
    ws: WizardState,
    default_validation_command: &str,
) -> RepoHealthWizardAnswers {
    let preferred_parallelism = ws
        .parallelism_input
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0 && *value <= 32)
        .unwrap_or(3);

    RepoHealthWizardAnswers {
        preferred_parallelism,
        validation_command: if ws.validation.trim().is_empty() {
            default_validation_command.to_string()
        } else {
            ws.validation
        },
        external_docs_accessible: ws.docs_accessible,
        backlog_approval: ws.backlog_approval,
        additional_context: ws.notes,
    }
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

fn draw_wizard_frame(frame: &mut ratatui::Frame<'_>, ws: &WizardState) {
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
        terminal
            .draw(|frame| draw_wizard_frame(frame, &ws))
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

    Ok(finalize_answers(ws, default_validation_command))
}

#[cfg(test)]
pub(crate) fn render_wizard_state(ws: &WizardState, width: u16, height: u16) -> String {
    super::render_to_string(width, height, |frame| draw_wizard_frame(frame, ws))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_at(step: usize) -> WizardState {
        WizardState {
            step,
            parallelism_input: "3".to_string(),
            validation: "./scripts/run-validate".to_string(),
            docs_accessible: true,
            backlog_approval: true,
            notes: String::new(),
        }
    }

    #[test]
    fn finalize_answers_defaults_invalid_parallelism_and_empty_validation() {
        let mut ws = state_at(4);
        ws.parallelism_input = "0".to_string();
        ws.validation = "   ".to_string();
        ws.docs_accessible = false;
        ws.backlog_approval = false;
        ws.notes = "prefer focused patches".to_string();

        let answers = finalize_answers(ws, "cargo test -p gardener");
        assert_eq!(answers.preferred_parallelism, 3);
        assert_eq!(answers.validation_command, "cargo test -p gardener");
        assert!(!answers.external_docs_accessible);
        assert!(!answers.backlog_approval);
        assert_eq!(answers.additional_context, "prefer focused patches");
    }

    #[test]
    fn finalize_answers_preserves_valid_parallelism_and_validation() {
        let mut ws = state_at(4);
        ws.parallelism_input = "12".to_string();
        ws.validation = "./scripts/run-validate".to_string();

        let answers = finalize_answers(ws, "cargo test");
        assert_eq!(answers.preferred_parallelism, 12);
        assert_eq!(answers.validation_command, "./scripts/run-validate");
        assert!(answers.external_docs_accessible);
        assert!(answers.backlog_approval);
    }

    #[test]
    fn finalize_answers_rejects_out_of_range_parallelism_and_keeps_non_empty_validation() {
        let mut ws = state_at(4);
        ws.parallelism_input = "33".to_string();
        ws.validation = " cargo test --workspace ".to_string();

        let answers = finalize_answers(ws, "./scripts/run-validate");
        assert_eq!(answers.preferred_parallelism, 3);
        assert_eq!(answers.validation_command, " cargo test --workspace ");
    }

    #[test]
    fn step_zero_handles_digits_backspace_enter_and_ignores_non_digits() {
        let mut ws = state_at(0);
        ws.parallelism_input.clear();
        ws.handle_key(KeyCode::Char('1'), KeyModifiers::NONE);
        ws.handle_key(KeyCode::Char('x'), KeyModifiers::NONE);
        ws.handle_key(KeyCode::Char('9'), KeyModifiers::CONTROL);
        ws.handle_key(KeyCode::Char('2'), KeyModifiers::NONE);
        assert_eq!(ws.parallelism_input, "12");
        assert_eq!(ws.step, 0);

        ws.handle_key(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(ws.parallelism_input, "1");

        assert_eq!(
            ws.handle_key(KeyCode::Enter, KeyModifiers::NONE),
            WizardAction::Continue
        );
        assert_eq!(ws.step, 1);
    }

    #[test]
    fn step_one_handles_text_editing_and_control_guards() {
        let mut ws = state_at(1);
        ws.validation.clear();
        ws.handle_key(KeyCode::Char('c'), KeyModifiers::NONE);
        ws.handle_key(KeyCode::Char('a'), KeyModifiers::NONE);
        ws.handle_key(KeyCode::Char('r'), KeyModifiers::NONE);
        ws.handle_key(KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert_eq!(ws.validation, "car");

        ws.handle_key(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(ws.validation, "ca");

        assert_eq!(
            ws.handle_key(KeyCode::Enter, KeyModifiers::NONE),
            WizardAction::Continue
        );
        assert_eq!(ws.step, 2);
    }

    #[test]
    fn step_two_toggles_docs_accessibility_and_advances() {
        let mut ws = state_at(2);
        ws.docs_accessible = true;
        ws.handle_key(KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(!ws.docs_accessible);

        ws.handle_key(KeyCode::Char('Y'), KeyModifiers::NONE);
        assert!(ws.docs_accessible);

        ws.handle_key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(ws.docs_accessible);

        assert_eq!(
            ws.handle_key(KeyCode::Enter, KeyModifiers::NONE),
            WizardAction::Continue
        );
        assert_eq!(ws.step, 3);
    }

    #[test]
    fn step_three_navigation_toggles_selection_and_shortcuts_finish() {
        let mut ws = state_at(3);
        ws.handle_key(KeyCode::Down, KeyModifiers::NONE);
        assert!(!ws.backlog_approval);
        assert_eq!(ws.step, 3);

        ws.handle_key(KeyCode::Tab, KeyModifiers::NONE);
        assert!(ws.backlog_approval);

        ws.handle_key(KeyCode::Up, KeyModifiers::NONE);
        assert!(!ws.backlog_approval);

        let mut auto = state_at(3);
        assert_eq!(
            auto.handle_key(KeyCode::Char('a'), KeyModifiers::NONE),
            WizardAction::Continue
        );
        assert!(!auto.backlog_approval);
        assert_eq!(auto.step, 4);

        let mut review = state_at(3);
        review.backlog_approval = false;
        assert_eq!(
            review.handle_key(KeyCode::Char('R'), KeyModifiers::NONE),
            WizardAction::Continue
        );
        assert!(review.backlog_approval);
        assert_eq!(review.step, 4);
    }

    #[test]
    fn notes_step_ignores_control_chars_and_escape_finishes() {
        let mut ws = state_at(4);
        ws.handle_key(KeyCode::Char('h'), KeyModifiers::NONE);
        ws.handle_key(KeyCode::Char('i'), KeyModifiers::NONE);
        ws.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(ws.notes, "hi");

        ws.handle_key(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(ws.notes, "h");

        assert_eq!(
            ws.handle_key(KeyCode::Esc, KeyModifiers::NONE),
            WizardAction::Finish
        );
    }

    #[test]
    fn render_wizard_state_covers_parallelism_validation_and_docs_steps() {
        let mut step0 = state_at(0);
        step0.parallelism_input = "7".to_string();
        let frame0 = render_wizard_state(&step0, 100, 20);
        assert!(frame0.contains("Worker parallelism"));
        assert!(frame0.contains("> 7"));
        assert!(frame0.contains("Step 1 of 5"));

        let mut step1 = state_at(1);
        step1.validation = "cargo test --workspace".to_string();
        let frame1 = render_wizard_state(&step1, 100, 20);
        assert!(frame1.contains("Validation command"));
        assert!(frame1.contains("cargo test --workspace"));
        assert!(frame1.contains("Step 2 of 5"));

        let mut step2 = state_at(2);
        step2.docs_accessible = false;
        let frame2 = render_wizard_state(&step2, 100, 20);
        assert!(frame2.contains("Architecture docs available?"));
        assert!(frame2.contains("> no"));
        assert!(frame2.contains("Step 3 of 5"));
    }

    #[test]
    fn render_wizard_state_covers_backlog_step_and_finish_footer() {
        let mut backlog = state_at(3);
        backlog.backlog_approval = false;
        let backlog_frame = render_wizard_state(&backlog, 100, 20);
        assert!(backlog_frame.contains("Backlog seeding"));
        assert!(backlog_frame.contains("auto-seed"));
        assert!(backlog_frame.contains("review tasks"));
        assert!(backlog_frame.contains("Step 4 of 5"));

        let mut notes = state_at(4);
        notes.notes = "ship with clean hooks".to_string();
        let notes_frame = render_wizard_state(&notes, 100, 20);
        assert!(notes_frame.contains("Additional constraints"));
        assert!(notes_frame.contains("ship with clean hooks"));
        assert!(notes_frame.contains("Enter to finish"));
    }
}
