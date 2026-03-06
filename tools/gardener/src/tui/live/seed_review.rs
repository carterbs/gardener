use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::errors::GardenerError;
use crate::seed_runner::SeedTask;
use crate::tui::state::seed_review::{
    finalize_review_decisions, handle_seed_review_input, ReviewDecision, SeedReviewInput,
    SeedReviewKey, SeedReviewState,
};
use crate::tui::terminal::teardown_terminal;
use crate::tui::views::seed_review::draw_seed_review_frame;

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
    let mut state = SeedReviewState::new(total);

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
                    state.input_mode,
                    &state.input_buffer,
                )
            })
            .map_err(|e| GardenerError::Io(e.to_string()))?;

        if let Event::Key(key) = event::read().map_err(|e| GardenerError::Io(e.to_string()))? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if handle_seed_review_input(&mut state, seed_review_input_from_key_event(key)) {
                break;
            }
        }
    }

    teardown_terminal(terminal)?;

    Ok(finalize_review_decisions(state.decisions))
}

fn seed_review_input_from_key_event(key: KeyEvent) -> SeedReviewInput {
    let mapped = match key.code {
        KeyCode::Enter => SeedReviewKey::Enter,
        KeyCode::Esc => SeedReviewKey::Escape,
        KeyCode::Backspace => SeedReviewKey::Backspace,
        KeyCode::Char(c) => SeedReviewKey::Char(c),
        _ => return SeedReviewInput {
            key: SeedReviewKey::Char('\0'),
            control: true,
        },
    };

    SeedReviewInput {
        key: mapped,
        control: key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL),
    }
}
