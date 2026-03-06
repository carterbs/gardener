use std::cell::RefCell;

use super::formatting::now_unix_millis;
use super::state::StartupHeadline;

const STARTUP_SPINNER_FRAMES: [&str; 6] = ["⠋", "⠙", "⠸", "⠴", "⠦", "⠇"];
const STARTUP_VERBS: [&str; 6] = [
    "Scanning",
    "Seeding",
    "Pruning",
    "Cultivating",
    "Grafting",
    "Harvesting",
];
const STARTUP_SPINNER_TICK_MS: u128 = 150;
const STARTUP_ELLIPSIS_TICK_MS: u128 = 400;
const STARTUP_SPINNER_TICKS: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StartupHeadlineView {
    pub(crate) spinner_frame: usize,
    pub(crate) startup_active: bool,
    pub(crate) ellipsis_phase: u8,
    verb_idx: usize,
}

impl StartupHeadlineView {
    pub(crate) fn from_tick(tick: u32, verb_idx: usize) -> Self {
        let max_tick = STARTUP_SPINNER_TICKS.saturating_sub(1);
        let startup_active = tick < STARTUP_SPINNER_TICKS;
        let spinner_tick = if startup_active { tick } else { max_tick };
        Self {
            spinner_frame: (spinner_tick as usize) % STARTUP_SPINNER_FRAMES.len(),
            startup_active,
            ellipsis_phase: ((tick / 3) % 3) as u8,
            verb_idx: verb_idx % STARTUP_VERBS.len(),
        }
    }

    pub(crate) fn from_elapsed_ms(elapsed_ms: u128, verb_idx: usize) -> Self {
        let spinner_tick = (elapsed_ms / STARTUP_SPINNER_TICK_MS) as u32;
        Self {
            ellipsis_phase: ((elapsed_ms / STARTUP_ELLIPSIS_TICK_MS) % 3) as u8,
            ..Self::from_tick(spinner_tick, verb_idx)
        }
    }

    pub(crate) fn spinner(self) -> &'static str {
        STARTUP_SPINNER_FRAMES[self.spinner_frame]
    }

    pub(crate) fn verb(self) -> &'static str {
        STARTUP_VERBS[self.verb_idx]
    }

    pub(crate) fn ellipsis(self) -> &'static str {
        match self.ellipsis_phase {
            0 => ".",
            1 => "..",
            _ => "...",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LiveStartupHeadlineState {
    started_at_ms: u128,
    verb_idx: usize,
}

thread_local! {
    static LIVE_STARTUP_HEADLINE: RefCell<Option<LiveStartupHeadlineState>> = const { RefCell::new(None) };
}

pub(super) fn live_startup_headline() -> StartupHeadlineView {
    LIVE_STARTUP_HEADLINE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let now = now_unix_millis();
            *slot = Some(LiveStartupHeadlineState {
                started_at_ms: now,
                verb_idx: (now as usize) % STARTUP_VERBS.len(),
            });
        }
        let state = slot.expect("live startup headline initialized");
        let now = now_unix_millis();
        let elapsed = now.saturating_sub(state.started_at_ms);
        StartupHeadlineView::from_elapsed_ms(elapsed, state.verb_idx)
    })
}

pub(super) fn reset_live_startup_headline() {
    LIVE_STARTUP_HEADLINE.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

impl StartupHeadline {
    pub(crate) fn from_view(source: StartupHeadlineView) -> Self {
        Self {
            spinner_frame: source.spinner_frame,
            verb: source.verb().to_string(),
            startup_active: source.startup_active,
            ellipsis_phase: source.ellipsis_phase,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_tick_clamps_after_startup_window_and_wraps_verb_index() {
        let view = StartupHeadlineView::from_tick(999, STARTUP_VERBS.len() + 2);
        let expected_frame = (STARTUP_SPINNER_TICKS.saturating_sub(1) as usize)
            % STARTUP_SPINNER_FRAMES.len();
        assert_eq!(view.spinner_frame, expected_frame);
        assert!(!view.startup_active);
        assert_eq!(view.ellipsis_phase, ((999 / 3) % 3) as u8);
        assert_eq!(view.verb(), STARTUP_VERBS[2]);
    }

    #[test]
    fn from_elapsed_ms_advances_spinner_and_ellipsis_independently() {
        let view = StartupHeadlineView::from_elapsed_ms(
            STARTUP_SPINNER_TICK_MS * 4 + STARTUP_ELLIPSIS_TICK_MS,
            1,
        );
        let expected_frame =
            ((STARTUP_SPINNER_TICK_MS * 4 + STARTUP_ELLIPSIS_TICK_MS) / STARTUP_SPINNER_TICK_MS)
                as usize
                % STARTUP_SPINNER_FRAMES.len();
        assert_eq!(view.spinner(), STARTUP_SPINNER_FRAMES[expected_frame]);
        assert_eq!(view.ellipsis(), "...");
        assert_eq!(view.verb(), "Seeding");
        assert!(view.startup_active);
    }

    #[test]
    fn from_view_copies_render_fields() {
        let view = StartupHeadlineView::from_tick(5, 3);
        let headline = StartupHeadline::from_view(view);
        assert_eq!(headline.spinner_frame, view.spinner_frame);
        assert_eq!(headline.ellipsis_phase, view.ellipsis_phase);
        assert_eq!(headline.startup_active, view.startup_active);
        assert_eq!(headline.verb, "Cultivating");
    }

    #[test]
    fn live_startup_headline_initializes_and_resets() {
        reset_live_startup_headline();
        let first = live_startup_headline();
        let second = live_startup_headline();
        assert_eq!(first.verb(), second.verb());
        assert!(first.startup_active);
        reset_live_startup_headline();
        let after_reset = live_startup_headline();
        assert!(STARTUP_VERBS.contains(&after_reset.verb()));
    }
}
