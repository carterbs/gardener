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

#[cfg(test)]
mod tests {
    use super::*;

    fn key_input(key: SeedReviewKey) -> SeedReviewInput {
        SeedReviewInput {
            key,
            control: false,
        }
    }

    fn controlled_char(c: char) -> SeedReviewInput {
        SeedReviewInput {
            key: SeedReviewKey::Char(c),
            control: true,
        }
    }

    #[test]
    fn new_initializes_empty_review_state() {
        let state = SeedReviewState::new(3);

        assert_eq!(state.decisions, vec![None, None, None]);
        assert_eq!(state.current, 0);
        assert_eq!(state.input_mode, None);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn input_mode_handler_is_noop_without_active_mode() {
        let mut state = SeedReviewState::new(1);

        apply_input_mode_key(&mut state, key_input(SeedReviewKey::Char('x')));

        assert_eq!(state, SeedReviewState::new(1));
    }

    #[test]
    fn discard_reason_enter_records_none_for_blank_buffer() {
        let mut state = SeedReviewState::new(2);
        state.input_mode = Some(InputMode::DiscardReason);
        state.input_buffer = "   ".into();

        apply_input_mode_key(&mut state, key_input(SeedReviewKey::Enter));

        assert_eq!(state.decisions[0], Some(ReviewDecision::Discard(None)));
        assert_eq!(state.current, 1);
        assert_eq!(state.input_mode, None);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn discard_reason_enter_records_provided_reason() {
        let mut state = SeedReviewState::new(1);
        state.input_mode = Some(InputMode::DiscardReason);
        state.input_buffer = "missing requirements".into();

        apply_input_mode_key(&mut state, key_input(SeedReviewKey::Enter));

        assert_eq!(
            state.decisions[0],
            Some(ReviewDecision::Discard(Some("missing requirements".into())))
        );
        assert_eq!(state.current, 1);
        assert_eq!(state.input_mode, None);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn refine_enter_ignores_blank_buffer_and_keeps_mode_active() {
        let mut state = SeedReviewState::new(1);
        state.input_mode = Some(InputMode::RefineFeedback);
        state.input_buffer = "  ".into();

        apply_input_mode_key(&mut state, key_input(SeedReviewKey::Enter));

        assert_eq!(state.decisions[0], None);
        assert_eq!(state.current, 0);
        assert_eq!(state.input_mode, Some(InputMode::RefineFeedback));
        assert_eq!(state.input_buffer, "  ");
    }

    #[test]
    fn refine_enter_records_feedback_and_advances() {
        let mut state = SeedReviewState::new(2);
        state.input_mode = Some(InputMode::RefineFeedback);
        state.input_buffer = "tighten acceptance criteria".into();

        apply_input_mode_key(&mut state, key_input(SeedReviewKey::Enter));

        assert_eq!(
            state.decisions[0],
            Some(ReviewDecision::Refine("tighten acceptance criteria".into()))
        );
        assert_eq!(state.current, 1);
        assert_eq!(state.input_mode, None);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn input_mode_escape_clears_buffer_and_exits_mode() {
        let mut state = SeedReviewState::new(1);
        state.input_mode = Some(InputMode::DiscardReason);
        state.input_buffer = "temporary".into();

        apply_input_mode_key(&mut state, key_input(SeedReviewKey::Escape));

        assert_eq!(state.input_mode, None);
        assert!(state.input_buffer.is_empty());
        assert_eq!(state.decisions[0], None);
    }

    #[test]
    fn input_mode_backspace_and_character_keys_update_buffer() {
        let mut state = SeedReviewState::new(1);
        state.input_mode = Some(InputMode::RefineFeedback);
        state.input_buffer = "ab".into();

        apply_input_mode_key(&mut state, key_input(SeedReviewKey::Backspace));
        apply_input_mode_key(&mut state, key_input(SeedReviewKey::Char('c')));
        apply_input_mode_key(&mut state, controlled_char('d'));

        assert_eq!(state.input_buffer, "ac");
    }

    #[test]
    fn review_mode_keep_records_decision_and_advances() {
        let mut state = SeedReviewState::new(2);

        let should_quit = apply_review_mode_key(&mut state, SeedReviewKey::Char('K'));

        assert!(!should_quit);
        assert_eq!(state.decisions[0], Some(ReviewDecision::Keep));
        assert_eq!(state.current, 1);
    }

    #[test]
    fn review_mode_switches_to_discard_and_refine_inputs() {
        let mut state = SeedReviewState::new(1);
        state.input_buffer = "stale".into();

        assert!(!apply_review_mode_key(&mut state, SeedReviewKey::Char('d')));
        assert_eq!(state.input_mode, Some(InputMode::DiscardReason));
        assert!(state.input_buffer.is_empty());

        state.input_buffer = "stale".into();
        assert!(!apply_review_mode_key(&mut state, SeedReviewKey::Char('R')));
        assert_eq!(state.input_mode, Some(InputMode::RefineFeedback));
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn review_mode_quit_keys_return_true_and_other_keys_do_not() {
        let mut state = SeedReviewState::new(1);

        assert!(apply_review_mode_key(&mut state, SeedReviewKey::Char('q')));
        assert!(apply_review_mode_key(&mut state, SeedReviewKey::Escape));
        assert!(!apply_review_mode_key(&mut state, SeedReviewKey::Backspace));
    }

    #[test]
    fn finalize_review_decisions_fills_missing_entries_with_discard_none() {
        let finalized = finalize_review_decisions(vec![
            Some(ReviewDecision::Keep),
            None,
            Some(ReviewDecision::Refine("more detail".into())),
        ]);

        assert_eq!(
            finalized,
            vec![
                ReviewDecision::Keep,
                ReviewDecision::Discard(None),
                ReviewDecision::Refine("more detail".into()),
            ]
        );
    }

    #[test]
    fn handle_seed_review_input_routes_by_mode() {
        let mut review_mode_state = SeedReviewState::new(1);
        let should_quit = handle_seed_review_input(
            &mut review_mode_state,
            key_input(SeedReviewKey::Char('q')),
        );
        assert!(should_quit);

        let mut input_mode_state = SeedReviewState::new(1);
        input_mode_state.input_mode = Some(InputMode::DiscardReason);
        input_mode_state.input_buffer = "duplicate".into();

        let should_quit = handle_seed_review_input(
            &mut input_mode_state,
            key_input(SeedReviewKey::Enter),
        );

        assert!(!should_quit);
        assert_eq!(
            input_mode_state.decisions[0],
            Some(ReviewDecision::Discard(Some("duplicate".into())))
        );
        assert_eq!(input_mode_state.current, 1);
    }
}
