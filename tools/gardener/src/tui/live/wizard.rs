use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::errors::GardenerError;
use crate::tui::state::wizard::{
    finalize_answers, RepoHealthWizardAnswers, WizardAction, WizardInput, WizardKey, WizardState,
};
use crate::tui::terminal::teardown_terminal;
use crate::tui::views::wizard::draw_wizard_frame;

pub fn run_repo_health_wizard(
    default_validation_command: &str,
) -> Result<RepoHealthWizardAnswers, GardenerError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(stdout, EnterAlternateScreen).map_err(|e| GardenerError::Io(e.to_string()))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| GardenerError::Io(e.to_string()))?;

    let mut state = WizardState::new(default_validation_command);

    loop {
        terminal
            .draw(|frame| draw_wizard_frame(frame, &state))
            .map_err(|e| GardenerError::Io(e.to_string()))?;

        if let Event::Key(key) = event::read().map_err(|e| GardenerError::Io(e.to_string()))? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if state.handle_input(wizard_input_from_key_event(key)) == WizardAction::Finish {
                break;
            }
        }
    }

    teardown_terminal(terminal)?;

    Ok(finalize_answers(state, default_validation_command))
}

fn wizard_input_from_key_event(key: KeyEvent) -> WizardInput {
    let mapped = match key.code {
        KeyCode::Esc => WizardKey::Escape,
        KeyCode::Enter => WizardKey::Enter,
        KeyCode::Backspace => WizardKey::Backspace,
        KeyCode::Up => WizardKey::Up,
        KeyCode::Down => WizardKey::Down,
        KeyCode::Tab => WizardKey::Tab,
        KeyCode::Char(c) => WizardKey::Char(c),
        _ => return WizardInput {
            key: WizardKey::Char('\0'),
            control: true,
        },
    };

    WizardInput {
        key: mapped,
        control: key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL),
    }
}
