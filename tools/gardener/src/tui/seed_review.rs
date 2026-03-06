use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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

struct SeedReviewState {
    decisions: Vec<Option<ReviewDecision>>,
    current: usize,
    input_mode: Option<InputMode>,
    input_buffer: String,
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
    render_to_string(width, height, |frame| {
        draw_seed_review_frame(
            frame,
            task,
            index,
            total,
            round,
            Some(&InputMode::DiscardReason),
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
    render_to_string(width, height, |frame| {
        draw_seed_review_frame(
            frame,
            task,
            index,
            total,
            round,
            Some(&InputMode::RefineFeedback),
            input_text,
        )
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
        let (label, hint) = input_mode_prompt(*mode);
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

fn review_counter(index: usize, total: usize, round: usize) -> String {
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

fn input_mode_prompt(mode: InputMode) -> (&'static str, Color) {
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

fn apply_input_mode_key(state: &mut SeedReviewState, code: KeyCode, modifiers: KeyModifiers) {
    let Some(mode) = state.input_mode else {
        return;
    };

    match code {
        KeyCode::Enter => match mode {
            InputMode::DiscardReason => {
                let reason = if state.input_buffer.trim().is_empty() {
                    None
                } else {
                    Some(state.input_buffer.clone())
                };
                state.decisions[state.current] = Some(ReviewDecision::Discard(reason));
                state.current += 1;
                state.input_mode = None;
                state.input_buffer.clear();
            }
            InputMode::RefineFeedback => {
                if !state.input_buffer.trim().is_empty() {
                    state.decisions[state.current] =
                        Some(ReviewDecision::Refine(state.input_buffer.clone()));
                    state.current += 1;
                    state.input_mode = None;
                    state.input_buffer.clear();
                }
            }
        },
        KeyCode::Esc => {
            state.input_mode = None;
            state.input_buffer.clear();
        }
        KeyCode::Backspace => {
            state.input_buffer.pop();
        }
        KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
            state.input_buffer.push(c);
        }
        _ => {}
    }
}

fn apply_review_mode_key(state: &mut SeedReviewState, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('k') | KeyCode::Char('K') => {
            state.decisions[state.current] = Some(ReviewDecision::Keep);
            state.current += 1;
            false
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            state.input_mode = Some(InputMode::DiscardReason);
            state.input_buffer.clear();
            false
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            state.input_mode = Some(InputMode::RefineFeedback);
            state.input_buffer.clear();
            false
        }
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => true,
        _ => false,
    }
}

fn finalize_review_decisions(decisions: Vec<Option<ReviewDecision>>) -> Vec<ReviewDecision> {
    decisions
        .into_iter()
        .map(|d| d.unwrap_or(ReviewDecision::Discard(None)))
        .collect()
}

fn handle_seed_review_key(state: &mut SeedReviewState, key: KeyEvent) -> bool {
    if key.kind != KeyEventKind::Press {
        return false;
    }

    if state.input_mode.is_some() {
        apply_input_mode_key(state, key.code, key.modifiers);
        false
    } else {
        apply_review_mode_key(state, key.code)
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
    let mut state = SeedReviewState {
        decisions: vec![None; total],
        current: 0,
        input_mode: None,
        input_buffer: String::new(),
    };

    loop {
        if state.current >= total {
            break;
        }
        terminal
            .draw(|frame| {
                draw_seed_review_frame(
                    frame,
                    &tasks[state.current],
                    state.current,
                    total,
                    round,
                    state.input_mode.as_ref(),
                    &state.input_buffer,
                )
            })
            .map_err(|e| GardenerError::Io(e.to_string()))?;

        if let Event::Key(key) = event::read().map_err(|e| GardenerError::Io(e.to_string()))? {
            if handle_seed_review_key(&mut state, key) {
                break;
            }
        }
    }

    teardown_terminal(terminal)?;

    Ok(finalize_review_decisions(state.decisions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventState};

    fn sample_task(priority: &str) -> SeedTask {
        SeedTask {
            title: "Improve startup diagnostics".to_string(),
            details: "Explain why validation blocks before worker launch.".to_string(),
            rationale: "Agents recover faster when startup failures are explicit.".to_string(),
            domain: "runtime".to_string(),
            priority: priority.to_string(),
        }
    }

    fn state_with_mode(mode: InputMode, input: &str) -> SeedReviewState {
        SeedReviewState {
            decisions: vec![None, None],
            current: 0,
            input_mode: Some(mode),
            input_buffer: input.to_string(),
        }
    }

    #[test]
    fn render_seed_review_shows_keep_discard_refine_actions() {
        let frame = render_seed_review(&sample_task("P2"), 0, 3, 100, 22);
        assert!(frame.contains("review backlog  (1/3)"));
        assert!(frame.contains("[k] Keep"));
        assert!(frame.contains("[d] Discard"));
        assert!(frame.contains("[r] Refine"));
        assert!(frame.contains("[q] Discard remaining & finish"));
    }

    #[test]
    fn render_seed_review_uses_priority_styles_for_p0_and_p1() {
        let p0 = render_seed_review(&sample_task("P0"), 0, 1, 90, 20);
        assert!(p0.contains("Priority: P0"));
        let p1 = render_seed_review(&sample_task("P1"), 0, 1, 90, 20);
        assert!(p1.contains("Priority: P1"));
        let p2 = render_seed_review(&sample_task("P2"), 0, 1, 90, 20);
        assert!(p2.contains("Priority: P2"));
    }

    #[test]
    fn review_counter_formats_initial_and_followup_rounds() {
        assert_eq!(review_counter(0, 3, 0), "review backlog  (1/3)");
        assert_eq!(review_counter(1, 4, 2), "review backlog  round 3  (2/4)");
    }

    #[test]
    fn input_mode_prompt_covers_discard_and_refine_labels() {
        assert_eq!(
            input_mode_prompt(InputMode::DiscardReason),
            (
                "Why discard? (optional — Enter to skip, Esc to cancel)",
                Color::Rgb(255, 122, 122)
            )
        );
        assert_eq!(
            input_mode_prompt(InputMode::RefineFeedback),
            (
                "How should this task change? (Enter to submit, Esc to cancel)",
                Color::Rgb(245, 196, 95)
            )
        );
    }

    #[test]
    fn discard_mode_enter_records_reason_and_advances() {
        let mut state = state_with_mode(InputMode::DiscardReason, "duplicate of startup lint");
        apply_input_mode_key(&mut state, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            state.decisions[0],
            Some(ReviewDecision::Discard(Some(
                "duplicate of startup lint".to_string()
            )))
        );
        assert_eq!(state.current, 1);
        assert_eq!(state.input_mode, None);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn discard_mode_enter_with_blank_reason_discards_without_reason() {
        let mut state = state_with_mode(InputMode::DiscardReason, "   ");
        apply_input_mode_key(&mut state, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(state.decisions[0], Some(ReviewDecision::Discard(None)));
        assert_eq!(state.current, 1);
    }

    #[test]
    fn refine_mode_requires_non_blank_feedback() {
        let mut blank = state_with_mode(InputMode::RefineFeedback, "   ");
        apply_input_mode_key(&mut blank, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(blank.current, 0);
        assert_eq!(blank.input_mode, Some(InputMode::RefineFeedback));

        let mut filled = state_with_mode(InputMode::RefineFeedback, "mention coverage gate");
        apply_input_mode_key(&mut filled, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            filled.decisions[0],
            Some(ReviewDecision::Refine(
                "mention coverage gate".to_string()
            ))
        );
        assert_eq!(filled.current, 1);
        assert_eq!(filled.input_mode, None);
    }

    #[test]
    fn input_mode_esc_backspace_and_ctrl_char_behave_correctly() {
        let mut state = state_with_mode(InputMode::RefineFeedback, "abc");
        apply_input_mode_key(&mut state, KeyCode::Backspace, KeyModifiers::NONE);
        apply_input_mode_key(&mut state, KeyCode::Char('d'), KeyModifiers::CONTROL);
        apply_input_mode_key(&mut state, KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(state.input_buffer, "abx");
        apply_input_mode_key(&mut state, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(state.input_mode, None);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn handle_seed_review_key_ignores_non_press_events() {
        let mut state = SeedReviewState {
            decisions: vec![None],
            current: 0,
            input_mode: None,
            input_buffer: String::new(),
        };
        let key = KeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert!(!handle_seed_review_key(&mut state, key));
        assert_eq!(state.current, 0);
        assert_eq!(state.decisions[0], None);
    }

    #[test]
    fn handle_seed_review_key_routes_input_mode_keys() {
        let mut state = state_with_mode(InputMode::DiscardReason, "done");
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(!handle_seed_review_key(&mut state, key));
        assert_eq!(
            state.decisions[0],
            Some(ReviewDecision::Discard(Some("done".to_string())))
        );
        assert_eq!(state.current, 1);
    }

    #[test]
    fn review_mode_keys_cover_keep_discard_refine_and_quit() {
        let mut keep = SeedReviewState {
            decisions: vec![None],
            current: 0,
            input_mode: None,
            input_buffer: String::new(),
        };
        assert!(!apply_review_mode_key(&mut keep, KeyCode::Char('K')));
        assert_eq!(keep.decisions[0], Some(ReviewDecision::Keep));
        assert_eq!(keep.current, 1);

        let mut discard = SeedReviewState {
            decisions: vec![None],
            current: 0,
            input_mode: None,
            input_buffer: "stale".to_string(),
        };
        assert!(!apply_review_mode_key(&mut discard, KeyCode::Char('d')));
        assert_eq!(discard.input_mode, Some(InputMode::DiscardReason));
        assert!(discard.input_buffer.is_empty());

        let mut refine = SeedReviewState {
            decisions: vec![None],
            current: 0,
            input_mode: None,
            input_buffer: "stale".to_string(),
        };
        assert!(!apply_review_mode_key(&mut refine, KeyCode::Char('R')));
        assert_eq!(refine.input_mode, Some(InputMode::RefineFeedback));
        assert!(refine.input_buffer.is_empty());

        let mut quit = SeedReviewState {
            decisions: vec![None],
            current: 0,
            input_mode: None,
            input_buffer: String::new(),
        };
        assert!(apply_review_mode_key(&mut quit, KeyCode::Esc));
        assert!(apply_review_mode_key(&mut quit, KeyCode::Char('q')));
    }

    #[test]
    fn finalize_review_decisions_discards_unreviewed_tasks() {
        let finalized = finalize_review_decisions(vec![
            Some(ReviewDecision::Keep),
            None,
            Some(ReviewDecision::Refine("narrow scope".to_string())),
        ]);
        assert_eq!(
            finalized,
            vec![
                ReviewDecision::Keep,
                ReviewDecision::Discard(None),
                ReviewDecision::Refine("narrow scope".to_string())
            ]
        );
    }
}
