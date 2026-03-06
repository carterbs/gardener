#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    Keep,
    Discard(Option<String>),
    Refine(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputMode {
    DiscardReason,
    RefineFeedback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeedReviewKey {
    Enter,
    Escape,
    Backspace,
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SeedReviewInput {
    pub(crate) key: SeedReviewKey,
    pub(crate) control: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeedReviewState {
    pub(crate) decisions: Vec<Option<ReviewDecision>>,
    pub(crate) current: usize,
    pub(crate) input_mode: Option<InputMode>,
    pub(crate) input_buffer: String,
}

impl SeedReviewState {
    pub(crate) fn new(total: usize) -> Self {
        Self {
            decisions: vec![None; total],
            current: 0,
            input_mode: None,
            input_buffer: String::new(),
        }
    }
}

pub(crate) fn apply_input_mode_key(
    state: &mut SeedReviewState,
    input: SeedReviewInput,
) {
    let Some(mode) = state.input_mode else {
        return;
    };

    match input.key {
        SeedReviewKey::Enter => match mode {
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
        SeedReviewKey::Escape => {
            state.input_mode = None;
            state.input_buffer.clear();
        }
        SeedReviewKey::Backspace => {
            state.input_buffer.pop();
        }
        SeedReviewKey::Char(c) if !input.control => {
            state.input_buffer.push(c);
        }
        _ => {}
    }
}

pub(crate) fn apply_review_mode_key(state: &mut SeedReviewState, key: SeedReviewKey) -> bool {
    match key {
        SeedReviewKey::Char('k') | SeedReviewKey::Char('K') => {
            state.decisions[state.current] = Some(ReviewDecision::Keep);
            state.current += 1;
            false
        }
        SeedReviewKey::Char('d') | SeedReviewKey::Char('D') => {
            state.input_mode = Some(InputMode::DiscardReason);
            state.input_buffer.clear();
            false
        }
        SeedReviewKey::Char('r') | SeedReviewKey::Char('R') => {
            state.input_mode = Some(InputMode::RefineFeedback);
            state.input_buffer.clear();
            false
        }
        SeedReviewKey::Char('q') | SeedReviewKey::Char('Q') | SeedReviewKey::Escape => true,
        _ => false,
    }
}

pub(crate) fn finalize_review_decisions(
    decisions: Vec<Option<ReviewDecision>>,
) -> Vec<ReviewDecision> {
    decisions
        .into_iter()
        .map(|decision| decision.unwrap_or(ReviewDecision::Discard(None)))
        .collect()
}

pub(crate) fn handle_seed_review_input(state: &mut SeedReviewState, input: SeedReviewInput) -> bool {
    if state.input_mode.is_some() {
        apply_input_mode_key(state, input);
        false
    } else {
        apply_review_mode_key(state, input.key)
    }
}
