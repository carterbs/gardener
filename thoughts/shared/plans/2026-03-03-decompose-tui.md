# Refactor: Split `tui.rs` (3672 lines) into `tui/` module directory

## Context

`tools/gardener/src/tui.rs` is 3672 lines containing at least seven distinct responsibility clusters: data model types, backlog parsing/ordering, dashboard rendering, triage rendering, seed review wizard, repo health wizard, and terminal lifecycle management. Breaking it into a `tui/` directory module with focused submodules improves navigability, reduces merge conflicts, and makes each concern independently comprehensible for AI coding agents.

## 1. Current state analysis

### Imports and constants (lines 1-86)

- All `use` declarations for crossterm, ratatui, std, and internal crates
- Constants: `WORKER_LIST_ROW_HEIGHT`, `COMPACT_WORKER_LIST_ROW_HEIGHT`, `RECENT_COMMAND_STREAM_LIMIT`, `WORKER_FLOW_STATES`, `STARTUP_SPINNER_FRAMES`, `STARTUP_VERBS`, `STARTUP_SPINNER_TICK_MS`, `STARTUP_ELLIPSIS_TICK_MS`, `STARTUP_SPINNER_TICKS`, `TRIAGE_STAGE_LABELS`, `WIZARD_STEP_LABELS`

### Public data model types (lines 34-220)

- `WorkerRow` (pub struct, 13 fields) -- input data from the runtime
- `QueueStats` (pub struct, 9 fields) -- queue counter snapshot
- `BacklogView` (pub struct, 2 fields) -- raw backlog strings
- `UiMode` (pub enum: Triage, Work)
- `TriageStage`, `StageState`, `TriageActivity`, `TriageArtifact` -- triage UI types
- `StartupHeadline` (pub struct) -- snapshot of startup spinner state
- `WorkerState` (pub enum + `from_str`) -- bucket enum for worker display
- `ActivityEntry`, `CommandEntry` -- per-worker log entries
- `WorkerCard` (pub struct) -- enriched worker display model
- `BacklogPriority` (pub enum + `span_style`) -- priority with color
- `BacklogItem` (pub struct) -- parsed backlog entry for display
- `AppState` (pub struct) -- full frame state, with `from_dashboard_feed` and `from_triage_feed` constructors

### Internal data model types (lines 411-461)

- `WorkerMetrics` (private struct + `from_app_state`)
- `ParsedBacklogPriority`, `ParsedBacklogItem` (private parsing types)

### Backlog parsing functions (lines 463-633)

- `parse_backlog_priority`, `dashboard_worker_rows_for_width`, `is_backlog_status_token`, `is_short_task_id`, `parse_backlog_item`, `is_in_progress_backlog_item`, `parse_merge_queue_item`, `ordered_backlog_items`, `ordered_merge_queue_items`, `backlog_items_with_capacity`

### Worker card rendering helpers (lines 635-706)

- `merge_worker_card_item` -- builds a `ListItem` for the merge worker

### Utility functions (lines 708-749)

- `parse_triage_artifact`, `now_hhmmss`, `run_context_summary`, `equipment_name_for_worker`

### Startup headline animation (lines 751-801)

- `StartupHeadlineView` (private struct + `from_tick`, `from_elapsed_ms`, `spinner`, `verb`, `ellipsis`)
- `LiveStartupHeadlineState` (private struct)

### Dashboard rendering (lines 803-1336)

- `render_dashboard` (pub) -- test-backend render entry point
- `render_dashboard_with_headline` (private) -- test-backend render with headline
- `render_dashboard_at_tick` (cfg(test)) -- tick-parameterized test render
- `draw_dashboard_frame` (private, ~435 lines) -- the main dashboard draw function; this is the largest single function in the file
- Lines 891-1336 are the body of `draw_dashboard_frame`

### Triage rendering (lines 1338-1443)

- `render_triage` (pub) -- test-backend render entry point
- `draw_triage_frame` (private)
- `draw_triage_frame_from_state` (private)

### Report view rendering (lines 1445-1493)

- `render_report_view` (pub) -- test-backend render
- `draw_report_frame` (private)

### Terminal lifecycle / thread-local state (lines 1495-2209)

- Thread-local statics: `LIVE_TUI`, `LIVE_TUI_SIZE`, `LIVE_STARTUP_HEADLINE`, `WORKERS_VIEWPORT_OFFSET`, `WORKERS_VIEWPORT_SELECTED`, `WORKERS_VIEWPORT_CAPACITY`, `WORKERS_TOTAL_COUNT`
- `now_unix_millis`, `live_startup_headline`
- `scroll_workers_down` (pub), `scroll_workers_up` (pub), `reset_workers_scroll` (pub)
- `draw_dashboard_live` (pub), `draw_report_live` (pub), `draw_seeding_live` (pub), `draw_triage_live` (pub), `draw_shutdown_screen_live` (pub)
- `with_live_terminal` (private) -- terminal init/resize helper
- `close_live_terminal` (pub)

### State label formatting (lines 2211-2439)

- `format_state_label` (pub(crate)) -- large match on FSM state strings
- `worker_flow_chain_spans` -- builds styled spans for worker flow chain
- `format_current_state_line`
- `worker_command_stream`, `command_stream_window`
- `normalize_worker_state` -- maps all FSM states to canonical flow stages
- `format_breadcrumb`, `format_breadcrumb_step` -- breadcrumb path formatting
- `to_title_case_words` -- generic utility

### Seeding screen rendering (lines 1636-1743)

- `draw_seeding_frame` (private)
- `render_seeding` (pub) -- test-backend render
- `render_seed_review` (pub) -- test-backend render
- `draw_seed_review_frame` (private, ~170 lines)

### Seed review wizard (lines 1917-2040)

- `ReviewDecision` (pub enum)
- `InputMode` (private enum)
- `run_seed_review_wizard` (pub) -- interactive event loop

### Shutdown screen (lines 2051-2147)

- `draw_shutdown_screen_live` (pub)
- `draw_shutdown_frame` (private)

### Repo health wizard (lines 2441-2747)

- `RepoHealthWizardAnswers` (pub struct)
- `WizardAction` (private enum)
- `WizardState` (private struct + `handle_key`)
- `run_repo_health_wizard` (pub) -- interactive event loop
- `wizard_step_indicator` (private, defined earlier at line 359)
- `triage_stages_with_state`, `triage_stage_progress` (private, defined at lines 319-357)
- `teardown_terminal` (private helper)

### Tests (lines 2749-3672)

- ~923 lines of tests covering all major areas

## 2. Proposed file structure

```
src/tui/
├── mod.rs                  (~80 lines)    thin orchestrator: submodule declarations + pub use re-exports
├── state.rs                (~310 lines)   AppState, WorkerCard, WorkerRow, QueueStats, BacklogView,
│                                          UiMode, WorkerState, WorkerMetrics, ActivityEntry,
│                                          CommandEntry, StartupHeadline, triage types,
│                                          from_dashboard_feed, from_triage_feed
├── backlog.rs              (~200 lines)   BacklogPriority, BacklogItem, ParsedBacklogPriority,
│                                          ParsedBacklogItem, all backlog parsing/ordering functions,
│                                          backlog_items_with_capacity
├── formatting.rs           (~280 lines)   format_state_label, normalize_worker_state,
│                                          worker_flow_chain_spans, format_current_state_line,
│                                          worker_command_stream, command_stream_window,
│                                          format_breadcrumb, format_breadcrumb_step,
│                                          to_title_case_words, truncate_right, now_hhmmss,
│                                          run_context_summary, equipment_name_for_worker,
│                                          merge_worker_card_item, wizard_step_indicator,
│                                          WORKER_FLOW_STATES
├── startup.rs              (~80 lines)    StartupHeadlineView, LiveStartupHeadlineState,
│                                          live_startup_headline, STARTUP_SPINNER_FRAMES,
│                                          STARTUP_VERBS, STARTUP_SPINNER_TICK_MS,
│                                          STARTUP_ELLIPSIS_TICK_MS, STARTUP_SPINNER_TICKS
├── dashboard.rs            (~500 lines)   draw_dashboard_frame, render_dashboard,
│                                          render_dashboard_with_headline, render_dashboard_at_tick,
│                                          dashboard_worker_rows_for_width,
│                                          WORKER_LIST_ROW_HEIGHT, COMPACT_WORKER_LIST_ROW_HEIGHT,
│                                          RECENT_COMMAND_STREAM_LIMIT
├── triage.rs               (~160 lines)   draw_triage_frame, draw_triage_frame_from_state,
│                                          render_triage, triage_stage_progress,
│                                          triage_stages_with_state, parse_triage_artifact,
│                                          TRIAGE_STAGE_LABELS
├── report.rs               (~60 lines)    draw_report_frame, render_report_view
├── seeding.rs              (~90 lines)    draw_seeding_frame, render_seeding
├── seed_review.rs          (~250 lines)   ReviewDecision, InputMode, draw_seed_review_frame,
│                                          render_seed_review, run_seed_review_wizard
├── wizard.rs               (~310 lines)   RepoHealthWizardAnswers, WizardAction, WizardState,
│                                          run_repo_health_wizard, WIZARD_STEP_LABELS
├── terminal.rs             (~200 lines)   thread-local statics (LIVE_TUI, LIVE_TUI_SIZE,
│                                          LIVE_STARTUP_HEADLINE, WORKERS_VIEWPORT_*),
│                                          with_live_terminal, close_live_terminal, teardown_terminal,
│                                          now_unix_millis, draw_dashboard_live, draw_report_live,
│                                          draw_seeding_live, draw_triage_live,
│                                          draw_shutdown_screen_live, draw_shutdown_frame,
│                                          scroll_workers_down, scroll_workers_up,
│                                          reset_workers_scroll
└── tests.rs                (~920 lines)   all #[cfg(test)] mod tests content
```

**Estimated total:** ~3240 lines across 13 files (reduction from 3672 due to eliminated redundant imports; each file has its own focused import block).

**Largest file:** `dashboard.rs` at ~500 lines. This is driven by `draw_dashboard_frame` being a single ~435-line function. Further decomposition of that function's body (e.g., extracting `render_now_card`, `render_workers_panel`, `render_backlog_panel`, `render_merge_queue_panel` helper functions) is recommended as a follow-up but is orthogonal to the file split.

## 3. Per-file rationale

### `mod.rs` (~80 lines)

Thin orchestrator. Declares all submodules and provides `pub use` re-exports to preserve the exact public API. No logic lives here.

### `state.rs` (~310 lines)

**What moves here:** `WorkerRow`, `QueueStats`, `BacklogView`, `UiMode`, `TriageStage`, `StageState`, `TriageActivity`, `TriageArtifact`, `StartupHeadline`, `WorkerState` (enum + `from_str`), `ActivityEntry`, `CommandEntry`, `WorkerCard`, `AppState` (struct + both constructors), `WorkerMetrics` (struct + `from_app_state`).

**Why:** These are the core data model types that flow between rendering functions. Keeping them together ensures a single place to understand the shape of data passed around. The `AppState` constructors reference backlog parsing and formatting functions -- these will use `super::backlog::*` and `super::formatting::*` imports.

### `backlog.rs` (~200 lines)

**What moves here:** `BacklogPriority`, `BacklogItem`, `ParsedBacklogPriority`, `ParsedBacklogItem`, `parse_backlog_priority`, `is_backlog_status_token`, `is_short_task_id`, `parse_backlog_item`, `is_in_progress_backlog_item`, `parse_merge_queue_item`, `ordered_backlog_items`, `ordered_merge_queue_items`, `backlog_items_with_capacity`.

**Why:** Backlog parsing is a self-contained concern. These functions parse raw strings into structured items and order them by priority. The only external dependency is on `BacklogPriority::span_style` which uses ratatui `Style`/`Color`. Isolating this enables focused testing of parsing logic.

### `formatting.rs` (~280 lines)

**What moves here:** `format_state_label`, `normalize_worker_state`, `worker_flow_chain_spans`, `format_current_state_line`, `worker_command_stream`, `command_stream_window`, `format_breadcrumb`, `format_breadcrumb_step`, `to_title_case_words`, `truncate_right`, `now_hhmmss`, `run_context_summary`, `equipment_name_for_worker`, `merge_worker_card_item`, `wizard_step_indicator`, `WORKER_FLOW_STATES`.

**Why:** These are all presentation-layer helpers that convert raw state strings, timestamps, and breadcrumbs into displayable text or styled spans. They are referenced by multiple rendering files (dashboard, triage, seed review, wizard) and form a natural shared utilities layer. Grouping them here avoids scattering formatting logic across multiple files.

### `startup.rs` (~80 lines)

**What moves here:** `StartupHeadlineView`, `LiveStartupHeadlineState`, `live_startup_headline`, and the `STARTUP_*` constants.

**Why:** The startup spinner animation is a self-contained concern with its own state machine (tick-based animation, freeze-after-N-ticks). It's referenced by `dashboard.rs` and `terminal.rs` but has no dependencies on other TUI code beyond `now_unix_millis`.

### `dashboard.rs` (~500 lines)

**What moves here:** `draw_dashboard_frame`, `render_dashboard`, `render_dashboard_with_headline`, `render_dashboard_at_tick`, `dashboard_worker_rows_for_width`, and the row-height / command-stream constants.

**Why:** The dashboard is the most complex single view. Giving it its own file makes it the clear owner of the main work-mode layout (header + now card + workers panel + backlog/merge queue). It imports from `state`, `backlog`, `formatting`, and `startup`.

### `triage.rs` (~160 lines)

**What moves here:** `draw_triage_frame`, `draw_triage_frame_from_state`, `render_triage`, `triage_stage_progress`, `triage_stages_with_state`, `parse_triage_artifact`, `TRIAGE_STAGE_LABELS`.

**Why:** The triage mode has its own layout, its own stage-progress state machine, and its own rendering. It is cleanly separable from the dashboard view.

### `report.rs` (~60 lines)

**What moves here:** `draw_report_frame`, `render_report_view`.

**Why:** The report view is the simplest rendering function. It's a small, self-contained screen with a header, body, and footer.

### `seeding.rs` (~90 lines)

**What moves here:** `draw_seeding_frame`, `render_seeding`.

**Why:** The seeding screen is another simple activity-list view, separate from the seed review wizard's interactive logic.

### `seed_review.rs` (~250 lines)

**What moves here:** `ReviewDecision`, `InputMode`, `draw_seed_review_frame`, `render_seed_review`, `run_seed_review_wizard`.

**Why:** The seed review wizard is an interactive event loop with its own input modes, key handling, and terminal setup/teardown. It's a natural unit of encapsulation. `ReviewDecision` is used by `startup.rs` (the caller), so it stays `pub`.

### `wizard.rs` (~310 lines)

**What moves here:** `RepoHealthWizardAnswers`, `WizardAction`, `WizardState` (struct + `handle_key`), `run_repo_health_wizard`, `WIZARD_STEP_LABELS`.

**Why:** The repo health wizard is the other interactive event loop. It has its own state machine (`WizardState::handle_key`), its own multi-step draw function, and its own answer struct. It's referenced only by `triage_interview.rs`.

### `terminal.rs` (~200 lines)

**What moves here:** All thread-local statics (`LIVE_TUI`, `LIVE_TUI_SIZE`, `LIVE_STARTUP_HEADLINE`, `WORKERS_VIEWPORT_OFFSET`, `WORKERS_VIEWPORT_SELECTED`, `WORKERS_VIEWPORT_CAPACITY`, `WORKERS_TOTAL_COUNT`), `with_live_terminal`, `close_live_terminal`, `teardown_terminal`, `now_unix_millis`, `draw_dashboard_live`, `draw_report_live`, `draw_seeding_live`, `draw_triage_live`, `draw_shutdown_screen_live`, `draw_shutdown_frame`, `scroll_workers_down`, `scroll_workers_up`, `reset_workers_scroll`.

**Why:** Terminal lifecycle management (init, resize, teardown) and the thread-local singleton pattern are an infrastructure concern distinct from widget rendering. The `draw_*_live` functions are thin wrappers that call `with_live_terminal` + the corresponding `draw_*_frame` function. The viewport scroll state is also terminal-level state, not widget logic.

### `tests.rs` (~920 lines)

**What moves here:** The entire `#[cfg(test)] mod tests` block.

**Why:** At 920+ lines, the tests are a significant portion of the file. Keeping them in a dedicated subfile improves readability of the implementation files. Since `#[cfg(test)]` modules can access private items of the parent module via `super::*`, and in a `tui/` directory the parent is `mod.rs`, the test file needs to be declared in `mod.rs` as `#[cfg(test)] mod tests;` and import from submodules via `super::submodule::item` or via re-exports in `mod.rs`. Some items currently tested are private -- these will need `pub(super)` visibility so the test file can access them.

**Alternative:** Distribute tests to the files they exercise (e.g., backlog tests in `backlog.rs`, formatting tests in `formatting.rs`). This is cleaner for unit tests but requires splitting the 920-line test block. The single-file approach is simpler for the initial migration; tests can be distributed in a follow-up.

## 4. Migration steps

### Phase 1: Create the directory and mod.rs

1. `mkdir src/tui`
2. `mv src/tui.rs src/tui/mod.rs` (git preserves history via `git mv`)
3. Verify `cargo check -p gardener` passes -- the module system resolves `tui/mod.rs` identically to `tui.rs`.

### Phase 2: Extract submodules (one at a time, `cargo check` between each)

Extract in dependency order (leaves first, so downstream files can import from already-extracted modules):

**Step 1: `formatting.rs`** -- pure functions, no dependencies on other TUI types except ratatui primitives and `crate::logging`.
- Move all formatting functions and `WORKER_FLOW_STATES`.
- Add `pub(super)` to functions used by other submodules.
- In `mod.rs`: `mod formatting;` and `pub(crate) use formatting::format_state_label;`
- `cargo check`

**Step 2: `startup.rs`** -- depends only on `now_unix_millis` (which will initially stay in mod.rs, then move to `terminal.rs` later).
- Move `StartupHeadlineView`, `LiveStartupHeadlineState`, `STARTUP_*` constants.
- `live_startup_headline` depends on thread-local `LIVE_STARTUP_HEADLINE` which stays in mod.rs for now.
- `cargo check`

**Step 3: `backlog.rs`** -- depends on ratatui types and `BacklogPriority::span_style`.
- Move all backlog types and parsing functions.
- `cargo check`

**Step 4: `state.rs`** -- depends on `backlog` and `formatting` submodules.
- Move all data model types.
- `AppState::from_dashboard_feed` calls `ordered_backlog_items`, `equipment_name_for_worker`, `now_hhmmss`, `format_breadcrumb` -- use `super::backlog::*` and `super::formatting::*`.
- `AppState::from_triage_feed` calls `triage_stage_progress`, `triage_stages_with_state`, `parse_triage_artifact` -- these move to `triage.rs` later, so initially use `super::*`.
- `cargo check`

**Step 5: `triage.rs`** -- depends on `state` types.
- Move triage rendering functions and triage stage helpers.
- `cargo check`

**Step 6: `report.rs`** -- depends on ratatui and `crate::hotkeys`.
- Move report rendering functions.
- `cargo check`

**Step 7: `seeding.rs`** -- depends on `formatting::now_hhmmss`.
- Move seeding frame rendering.
- `cargo check`

**Step 8: `seed_review.rs`** -- depends on `crate::seed_runner::SeedTask` and crossterm event handling.
- Move `ReviewDecision`, `InputMode`, the draw function, and the interactive wizard.
- `cargo check`

**Step 9: `wizard.rs`** -- depends on crossterm event handling and `formatting::wizard_step_indicator`.
- Move `RepoHealthWizardAnswers`, `WizardAction`, `WizardState`, `run_repo_health_wizard`, `WIZARD_STEP_LABELS`.
- `cargo check`

**Step 10: `dashboard.rs`** -- depends on `state`, `backlog`, `formatting`, `startup`.
- Move `draw_dashboard_frame` and its render entry points.
- This is the most complex extraction due to the large function body referencing many helpers.
- `cargo check`

**Step 11: `terminal.rs`** -- depends on all `draw_*_frame` functions.
- Move all thread-local statics, `with_live_terminal`, `close_live_terminal`, `teardown_terminal`, `now_unix_millis`.
- Move `draw_*_live` wrappers, scroll functions, and `draw_shutdown_frame`/`draw_shutdown_screen_live`.
- `cargo check`

**Step 12: `tests.rs`** -- final step.
- Move the `#[cfg(test)] mod tests` block to `tests.rs`.
- In `mod.rs`: add `#[cfg(test)] mod tests;`
- Adjust imports: `use super::*` becomes more specific imports from submodules.
- Mark private items needed by tests as `pub(super)`.
- `cargo check && cargo test -p gardener`

### Phase 3: Final verification

```bash
cargo check -p gardener
cargo test -p gardener
cargo clippy -p gardener
```

## 5. Public API preservation

All external consumers must see zero changes. The `mod.rs` re-export block:

```rust
mod backlog;
mod dashboard;
mod formatting;
mod report;
mod seed_review;
mod seeding;
mod startup;
mod state;
mod terminal;
mod triage;
mod wizard;

// Public types
pub use state::{
    ActivityEntry, AppState, BacklogView, CommandEntry, QueueStats, StartupHeadline,
    TriageActivity, TriageArtifact, TriageStage, StageState, UiMode, WorkerCard,
    WorkerRow, WorkerState,
};
pub use backlog::{BacklogItem, BacklogPriority};
pub use seed_review::ReviewDecision;
pub use wizard::RepoHealthWizardAnswers;

// Public functions used by runtime/worker_pool/startup
pub use dashboard::render_dashboard;
pub use report::render_report_view;
pub use seed_review::run_seed_review_wizard;
pub use seeding::render_seeding;
pub use terminal::{
    close_live_terminal, draw_dashboard_live, draw_report_live, draw_seeding_live,
    draw_shutdown_screen_live, draw_triage_live, reset_workers_scroll, scroll_workers_down,
    scroll_workers_up,
};
pub use triage::render_triage;
pub use wizard::run_repo_health_wizard;

// pub(crate) items
pub(crate) use formatting::format_state_label;

#[cfg(test)]
mod tests;
```

### Consumers (verified):

| Consumer | Imports | Satisfied by |
|---|---|---|
| `worker_pool.rs` | `format_state_label`, `reset_workers_scroll`, `scroll_workers_down`, `scroll_workers_up`, `BacklogView`, `QueueStats`, `WorkerRow` | `formatting`, `terminal`, `state` via mod.rs re-exports |
| `runtime/mod.rs` | `close_live_terminal`, `draw_dashboard_live`, `draw_report_live`, `draw_triage_live`, `render_dashboard`, `render_triage`, `BacklogView`, `QueueStats`, `WorkerRow` | `terminal`, `dashboard`, `triage`, `state` via mod.rs re-exports |
| `runtime/mod.rs` (path-qualified) | `crate::tui::render_report_view`, `crate::tui::render_seeding`, `crate::tui::draw_seeding_live`, `crate::tui::draw_shutdown_screen_live` | `report`, `seeding`, `terminal` via mod.rs re-exports |
| `triage_interview.rs` | `run_repo_health_wizard` | `wizard` via mod.rs re-export |
| `startup.rs` (path-qualified) | `crate::tui::run_seed_review_wizard`, `crate::tui::ReviewDecision` | `seed_review` via mod.rs re-exports |

## 6. Risk assessment

### Tightly coupled: `draw_dashboard_frame` and thread-local viewport state

The `draw_dashboard_frame` function (lines 891-1336) directly reads and writes to thread-local statics `WORKERS_VIEWPORT_OFFSET`, `WORKERS_VIEWPORT_SELECTED`, `WORKERS_VIEWPORT_CAPACITY`, and `WORKERS_TOTAL_COUNT`. These thread-locals also live in the terminal lifecycle code. After the split, `dashboard.rs` needs to call into `terminal.rs` to access viewport state.

**Mitigation:** Expose accessor functions in `terminal.rs`:
```rust
pub(super) fn get_workers_viewport_selected() -> usize { ... }
pub(super) fn set_workers_viewport_capacity(cap: usize) { ... }
pub(super) fn set_workers_total_count(count: usize) { ... }
pub(super) fn update_workers_viewport_offset(selected: usize, capacity: usize) -> usize { ... }
```

Alternatively, pass viewport state as a mutable parameter to `draw_dashboard_frame` instead of using thread-locals. This would be a behavioral improvement but increases the scope of the refactor.

### Tightly coupled: `AppState::from_triage_feed` and triage helpers

`AppState::from_triage_feed` (in `state.rs`) calls `triage_stage_progress` and `triage_stages_with_state` (which move to `triage.rs`), and `parse_triage_artifact` (also moves to `triage.rs`). This creates a circular dependency: `state` -> `triage` and `triage` -> `state` (for `AppState`).

**Mitigation:** Keep the `from_triage_feed` constructor in `triage.rs` as a free function or move the triage-specific helpers (`triage_stage_progress`, `triage_stages_with_state`, `parse_triage_artifact`) into `state.rs` alongside `AppState`. The recommended approach is to move `from_triage_feed` to be a function in `triage.rs` that returns an `AppState`, breaking the cycle: `triage.rs` depends on `state.rs` types but `state.rs` does not depend on `triage.rs`.

### Tightly coupled: `AppState::from_dashboard_feed` and backlog/formatting

Similar to the triage case. `from_dashboard_feed` calls into `backlog::ordered_backlog_items` and `formatting::*`. This is a one-directional dependency (state -> backlog, state -> formatting) so there is no cycle. No mitigation needed.

### Private items needed by tests

The test module accesses private types and functions: `WorkerMetrics`, `StartupHeadlineView`, `command_stream_window`, `worker_command_stream`, `worker_flow_chain_spans`, `format_breadcrumb`, `render_dashboard_at_tick`, `WizardAction`, `WizardState`. After the split, these live in different submodules.

**Mitigation:** Make these `pub(super)` so the test file (a sibling module in `tui/`) can import them. This is the standard Rust pattern for testing private internals. The items remain invisible to code outside `tui/`.

### `now_unix_millis` usage across modules

`now_unix_millis` is used by `now_hhmmss` (formatting) and `live_startup_headline` (startup/terminal). It's a tiny pure function.

**Mitigation:** Place it in `terminal.rs` (alongside the thread-local state that uses it) and make it `pub(super)`. Or duplicate it -- it's a two-line function. Placing it in `terminal.rs` is cleaner.

### Test file size

At ~920 lines, `tests.rs` is near the upper bound of the target range. In a follow-up, tests should be distributed to the files they exercise (backlog parsing tests in `backlog.rs`, formatting tests in `formatting.rs`, wizard key handling tests in `wizard.rs`, etc.). This is not required for the initial migration.

## 7. Follow-up improvements (out of scope for this plan)

1. **Decompose `draw_dashboard_frame`:** Extract helper functions (`render_now_card`, `render_workers_panel`, `render_backlog_panel`, `render_merge_queue_panel`) to reduce the ~435-line function body.
2. **Eliminate thread-local viewport state:** Pass viewport state as parameters instead of using `thread_local!` statics. This makes the code more testable and removes hidden mutable state.
3. **Distribute tests:** Move test functions from the monolithic `tests.rs` to inline `#[cfg(test)] mod tests` blocks in each submodule file.
4. **Extract a `colors.rs` or `theme.rs`:** Many files repeat `Color::Rgb(85, 198, 255)` (Gardener blue), `Color::Rgb(245, 196, 95)` (Gardener gold), `Color::Rgb(82, 88, 126)` (border gray), etc. A shared palette would reduce duplication and make theme changes trivial.
