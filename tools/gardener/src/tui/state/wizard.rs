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
                WizardKey::Char(c) if !input.control => {
                    self.validation.push(c)
                }
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
                WizardKey::Char(c) if !input.control => {
                    self.notes.push(c)
                }
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
