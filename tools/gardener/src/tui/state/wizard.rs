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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WizardKey {
    Escape,
    Enter,
    Backspace,
    Up,
    Down,
    Tab,
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WizardInput {
    pub(crate) key: WizardKey,
    pub(crate) control: bool,
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
    pub(crate) fn new(default_validation_command: &str) -> Self {
        Self {
            step: 0,
            parallelism_input: "3".to_string(),
            validation: default_validation_command.to_string(),
            docs_accessible: true,
            backlog_approval: true,
            notes: String::new(),
        }
    }

    pub(crate) fn handle_input(&mut self, input: WizardInput) -> WizardAction {
        if input.key == WizardKey::Escape {
            return WizardAction::Finish;
        }
        match self.step {
            0 => match input.key {
                WizardKey::Enter => self.step = 1,
                WizardKey::Backspace => {
                    self.parallelism_input.pop();
                }
                WizardKey::Char(c) if !input.control && c.is_ascii_digit() => {
                    self.parallelism_input.push(c)
                }
                _ => {}
            },
            1 => match input.key {
                WizardKey::Enter => self.step = 2,
                WizardKey::Backspace => {
                    self.validation.pop();
                }
                WizardKey::Char(c) if !input.control => self.validation.push(c),
                _ => {}
            },
            2 => match input.key {
                WizardKey::Enter => self.step = 3,
                WizardKey::Char('y') | WizardKey::Char('Y') => self.docs_accessible = true,
                WizardKey::Char('n') | WizardKey::Char('N') => self.docs_accessible = false,
                _ => {}
            },
            3 => match input.key {
                WizardKey::Enter => self.step = 4,
                WizardKey::Char('a') | WizardKey::Char('A') => {
                    self.backlog_approval = false;
                    self.step = 4;
                }
                WizardKey::Char('r') | WizardKey::Char('R') => {
                    self.backlog_approval = true;
                    self.step = 4;
                }
                WizardKey::Up | WizardKey::Down | WizardKey::Tab => {
                    self.backlog_approval = !self.backlog_approval
                }
                _ => {}
            },
            _ => match input.key {
                WizardKey::Enter => return WizardAction::Finish,
                WizardKey::Backspace => {
                    self.notes.pop();
                }
                WizardKey::Char(c) if !input.control => self.notes.push(c),
                _ => {}
            },
        }
        WizardAction::Continue
    }
}

pub(crate) fn finalize_answers(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn input(key: WizardKey) -> WizardInput {
        WizardInput {
            key,
            control: false,
        }
    }

    fn control_input(key: WizardKey) -> WizardInput {
        WizardInput { key, control: true }
    }

    #[test]
    fn new_state_uses_expected_defaults() {
        let state = WizardState::new("cargo test");

        assert_eq!(state.step, 0);
        assert_eq!(state.parallelism_input, "3");
        assert_eq!(state.validation, "cargo test");
        assert!(state.docs_accessible);
        assert!(state.backlog_approval);
        assert!(state.notes.is_empty());
    }

    #[test]
    fn escape_finishes_from_any_step() {
        let mut state = WizardState::new("cargo test");
        state.step = 3;

        let action = state.handle_input(input(WizardKey::Escape));

        assert_eq!(action, WizardAction::Finish);
        assert_eq!(state.step, 3);
    }

    #[test]
    fn step_zero_edits_parallelism_and_advances() {
        let mut state = WizardState::new("cargo test");

        assert_eq!(
            state.handle_input(input(WizardKey::Backspace)),
            WizardAction::Continue
        );
        assert_eq!(state.parallelism_input, "");

        state.handle_input(input(WizardKey::Char('4')));
        state.handle_input(control_input(WizardKey::Char('8')));
        state.handle_input(input(WizardKey::Char('x')));
        assert_eq!(state.parallelism_input, "4");

        state.handle_input(input(WizardKey::Enter));
        assert_eq!(state.step, 1);
    }

    #[test]
    fn step_one_edits_validation_and_advances() {
        let mut state = WizardState::new("cargo test");
        state.step = 1;
        state.validation.clear();

        state.handle_input(input(WizardKey::Char('m')));
        state.handle_input(input(WizardKey::Char('a')));
        state.handle_input(control_input(WizardKey::Char('x')));
        state.handle_input(input(WizardKey::Backspace));
        state.handle_input(input(WizardKey::Enter));

        assert_eq!(state.validation, "m");
        assert_eq!(state.step, 2);
    }

    #[test]
    fn step_two_updates_docs_access_and_advances() {
        let mut state = WizardState::new("cargo test");
        state.step = 2;

        state.handle_input(input(WizardKey::Char('n')));
        assert!(!state.docs_accessible);

        state.handle_input(input(WizardKey::Char('Y')));
        assert!(state.docs_accessible);

        state.handle_input(input(WizardKey::Char('q')));
        assert!(state.docs_accessible);

        state.handle_input(input(WizardKey::Enter));
        assert_eq!(state.step, 3);
    }

    #[test]
    fn step_three_supports_toggle_shortcuts_and_selection() {
        let mut state = WizardState::new("cargo test");
        state.step = 3;

        state.handle_input(input(WizardKey::Up));
        assert!(!state.backlog_approval);
        assert_eq!(state.step, 3);

        state.handle_input(input(WizardKey::Down));
        assert!(state.backlog_approval);

        state.handle_input(input(WizardKey::Tab));
        assert!(!state.backlog_approval);
        assert_eq!(state.step, 3);

        state.handle_input(input(WizardKey::Char('r')));
        assert!(state.backlog_approval);
        assert_eq!(state.step, 4);
    }

    #[test]
    fn step_three_allows_auto_approval_and_enter_advance() {
        let mut state = WizardState::new("cargo test");
        state.step = 3;

        state.handle_input(input(WizardKey::Char('a')));
        assert!(!state.backlog_approval);
        assert_eq!(state.step, 4);

        state.step = 3;
        state.backlog_approval = false;
        state.handle_input(input(WizardKey::Enter));
        assert_eq!(state.step, 4);
        assert!(!state.backlog_approval);
    }

    #[test]
    fn notes_step_edits_text_and_enter_finishes() {
        let mut state = WizardState::new("cargo test");
        state.step = 4;

        state.handle_input(input(WizardKey::Char('h')));
        state.handle_input(control_input(WizardKey::Char('i')));
        state.handle_input(input(WizardKey::Char('i')));
        state.handle_input(input(WizardKey::Backspace));
        assert_eq!(state.notes, "h");

        let action = state.handle_input(input(WizardKey::Enter));
        assert_eq!(action, WizardAction::Finish);
        assert_eq!(state.notes, "h");
    }

    #[test]
    fn finalize_answers_uses_defaults_for_invalid_parallelism_and_blank_validation() {
        let answers = finalize_answers(
            WizardState {
                step: 4,
                parallelism_input: "0".to_string(),
                validation: "   ".to_string(),
                docs_accessible: false,
                backlog_approval: false,
                notes: "extra".to_string(),
            },
            "cargo test -p gardener",
        );

        assert_eq!(answers.preferred_parallelism, 3);
        assert_eq!(answers.validation_command, "cargo test -p gardener");
        assert!(!answers.external_docs_accessible);
        assert!(!answers.backlog_approval);
        assert_eq!(answers.additional_context, "extra");
    }

    #[test]
    fn finalize_answers_accepts_valid_parallelism_and_custom_validation() {
        let answers = finalize_answers(
            WizardState {
                step: 4,
                parallelism_input: "32".to_string(),
                validation: "cargo nextest run".to_string(),
                docs_accessible: true,
                backlog_approval: true,
                notes: String::new(),
            },
            "cargo test -p gardener",
        );

        assert_eq!(answers.preferred_parallelism, 32);
        assert_eq!(answers.validation_command, "cargo nextest run");
        assert!(answers.external_docs_accessible);
        assert!(answers.backlog_approval);
    }
}
