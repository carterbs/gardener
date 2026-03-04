# Refactor: Split `worker_pool.rs` (2354 lines) into `worker_pool/` module directory

## Context

`tools/gardener/src/worker_pool.rs` is 2354 lines containing the entire worker pool finite state machine: initialization, task claiming/scheduling, doing-worker result handling, merge-worker result handling, TUI event processing, hotkey handling, dashboard rendering, state transition logic, and a substantial test suite. Breaking it into a `worker_pool/` directory module with focused sub-files improves comprehension for AI coding agents and reduces merge conflicts when multiple agents touch worker pool concerns simultaneously.

## Current state analysis

### Imports and constants (lines 1-36)

26 `use` statements pulling from `crate::*`, `serde_json`, and `std`. Six module-level constants:
- `WORKER_POOL_ID`, `MERGE_WORKER_ID`, `WORKER_COMMAND_HISTORY_LIMIT`, `COPY_SHORTCUT_KEY`, `IDLE_WORKER_POLL_DELAY_MS`, `IDLE_WORKER_POLL_ATTEMPTS`

### Internal types (lines 37-105)

- `enum PoolResultMessage` (lines 39-49) -- channel message sum type for doing/merge results
- `enum PoolStreamEvent` (lines 51-64) -- TUI streaming events (ToolCommand, StateChanged)
- `struct ShutdownSummary` + `impl ShutdownSummary::format_message` (lines 66-94) -- end-of-run summary
- `struct HotkeyState<'a>` (lines 96-105) -- parameter bundle for hotkey handler

### `run_worker_pool_fsm` -- the main FSM function (lines 107-1137)

This is the largest single function in the file at ~1030 lines. It contains:

1. **Initialization & worker row setup** (lines 107-179): function signature, clearing interrupts, resetting scroll, logging, creating `WorkerRow` vec, merge row, counters, timestamps.

2. **Claim closures** (lines 182-251): Two closures defined inline:
   - `claim_tasks_for_available_workers` (lines 182-231) -- iterates idle slots, calls `store.claim_next`, updates TUI rows
   - `mark_merge_worker_busy` (lines 233-251) -- updates merge row TUI state

3. **Outer loop: epoch iteration** (lines 255-1103): `while completed < target` loop that:
   - Calls `handle_hotkeys` for quit detection (lines 256-270)
   - Creates worktree client, defines `maybe_start_merge` closure (lines 271-337)
   - Claims tasks and starts merge (lines 338-374)
   - Idle polling with retry (lines 376-395)
   - Sets up channels (`tx`/`rx` for results, `event_tx`/`event_rx` for TUI, `merge_tx`/`merge_rx` for merge requests) (lines 348-360)
   - **`std::thread::scope` block** (lines 406-1056): spawns doing worker threads and merge worker thread, then runs the inner event loop:
     - **Doing worker spawn** (lines 408-460)
     - **Merge worker thread spawn** (lines 462-508)
     - **Inner event loop** (`while active_doing > 0 || active_merging > 0`) (lines 514-1054):
       - Drains pool events (line 515-520)
       - Handles hotkeys for quit during active work (lines 521-546)
       - `rx.recv_timeout` match on `DoingResult` (lines 548-868) -- handles errors, HandoffToMerge, Completed, Failed outcomes, re-claims next task
       - `rx.recv_timeout` match on `MergeResult` (lines 870-994) -- handles merge errors, interrupted, completed, failed outcomes
       - Timeout branch: dashboard refresh, heartbeat logging, shutdown logging (lines 995-1048)
       - Disconnected branch (lines 1049-1052)

4. **Post-loop shutdown** (lines 1058-1137): builds `ShutdownSummary`, handles error/quit/normal completion screens, calls `wait_for_quit`.

### Event processing functions (lines 1139-1218)

- `apply_pool_stream_event` (lines 1139-1202) -- applies a single `PoolStreamEvent` to worker TUI rows
- `drain_pool_events` (lines 1204-1218) -- drains all pending events from the channel

### Wait-for-quit function (lines 1220-1294)

- `wait_for_quit` -- polls terminal for key input, handles copy-to-clipboard on Ctrl+C or 'c', exits on any key

### Hotkey handler (lines 1296-1436)

- `handle_hotkeys` (lines 1296-1436) -- polls for key, dispatches to action (quit, scroll, retry, release-lease, park/escalate, view/regenerate report, back), renders dashboard or report overlay

### Utility functions (lines 1438-1698)

- `now_unix_millis` (lines 1438-1443) -- current time helper
- `append_worker_command` (lines 1445-1453) -- appends command to worker history ring buffer
- `execution_task_packet` (lines 1455-1468) -- builds task summary string for worker execution
- `set_worker_idle` (lines 1470-1481) -- resets worker row to idle state
- `refresh_worker_heartbeats` (lines 1483-1492) -- updates heartbeat/session age on all workers
- `now_hhmmss` (lines 1494-1504) -- formats current time as HH:MM:SS
- `is_non_regressive_state_transition` (lines 1506-1520) -- validates state machine forward-only transitions
- `worker_state_rank` (lines 1522-1537) -- assigns numeric rank to worker states
- `normalize_worker_state_for_transition` (lines 1539-1571) -- canonicalizes raw state strings to normalized state names
- `is_copy_shortcut_key` (lines 1573-1575) -- checks if key matches copy shortcut
- `hotkey_action` (lines 1577-1579) -- delegates to `action_for_key_with_mode`
- `struct DashboardSnapshot` + `fn dashboard_snapshot` (lines 1581-1646) -- queries backlog store and builds TUI dashboard data
- `fn render` (lines 1648-1673) -- renders dashboard (TTY) or structured fallback lines (non-TTY)
- `fn short_task_id` (lines 1675-1677) -- truncates task ID to 6 chars
- `fn worker_failure_prompt` (lines 1679-1689) -- builds failure prompt for error screen
- `fn quality_report_path` (lines 1691-1698) -- resolves quality report file path

### Test module (lines 1700-2354)

655 lines of tests:
- Test helpers: `seed_task`, `test_scope`, `write_file` (lines 1723-1752)
- `execution_task_packet_includes_details_when_present` (lines 1754-1783)
- `report_hotkey_actions_cover_report_bindings` (lines 1785-1791)
- `hotkey_actions_match_default_and_operator_contracts` (lines 1793-1814)
- `all_advertised_hotkeys_have_actions` (lines 1816-1824)
- `run_worker_pool_fsm_switches_between_dashboard_and_report_frames` (lines 1826-1875)
- `run_worker_pool_fsm_handles_v_and_b_with_report_draws` (lines 1877-1920)
- `run_worker_pool_fsm_handles_g_and_regenerates_report` (lines 1922-1968)
- `run_worker_pool_fsm_claims_tasks_inserted_while_idle` (lines 1970-2036)
- `run_worker_pool_fsm_quits_on_q` (lines 2038-2066)
- `wait_for_quit_copies_error_on_ctrl_c` (lines 2068-2081)
- `wait_for_quit_does_not_copy_without_target` (lines 2083-2089)
- `wait_for_quit_copies_error_on_copy_shortcut` (lines 2091-2101)
- `wait_for_quit_copies_error_on_copy_shortcut_uppercase` (lines 2103-2113)
- `wait_for_quit_does_not_copy_error_on_other_key` (lines 2115-2122)
- `run_worker_pool_fsm_ignores_operator_hotkeys_by_default` (lines 2124-2166)
- `state_transition_guard_prevents_handoff_regression` (lines 2168-2177)
- `apply_pool_stream_event_updates_doing_worker_from_live_events` (lines 2179-2258)
- `run_worker_pool_limits_worker_slots_to_target` (lines 2260-2292)
- `worker_execute_dispatch_includes_insert_awareness_metadata` (lines 2294-2353)

## Public API (unchanged)

Only one consumer imports from `crate::worker_pool`:
- `lib.rs`: `use worker_pool::run_worker_pool_fsm;` (the module is `pub mod worker_pool`)

The sole public item is `pub fn run_worker_pool_fsm`. All re-exports stay in `worker_pool/mod.rs` -- zero changes to consumers.

## Proposed file structure

```
src/worker_pool/
├── mod.rs               (~250 lines)  struct definitions, constants, run_worker_pool_fsm shell,
│                                      initialization, outer loop skeleton, shutdown handling,
│                                      pub use re-export
├── event_handling.rs    (~180 lines)  apply_pool_stream_event, drain_pool_events,
│                                      is_non_regressive_state_transition, worker_state_rank,
│                                      normalize_worker_state_for_transition
├── hotkeys.rs           (~170 lines)  handle_hotkeys, HotkeyState, hotkey_action,
│                                      is_copy_shortcut_key, wait_for_quit
├── dashboard.rs         (~120 lines)  DashboardSnapshot, dashboard_snapshot, render,
│                                      short_task_id, quality_report_path,
│                                      worker_failure_prompt
├── scheduling.rs        (~200 lines)  claim_tasks_for_available_workers (extracted from closure),
│                                      mark_merge_worker_busy (extracted from closure),
│                                      maybe_start_merge (extracted from closure),
│                                      set_worker_idle, append_worker_command,
│                                      execution_task_packet, refresh_worker_heartbeats
├── result_handling.rs   (~500 lines)  handle_doing_result, handle_merge_result,
│                                      run_inner_event_loop (the while active_doing/merging loop),
│                                      spawn_doing_workers, spawn_merge_worker
├── util.rs              (~30 lines)   now_unix_millis, now_hhmmss, ShutdownSummary
└── tests.rs             (~660 lines)  all #[cfg(test)] tests + helpers
```

Estimated total: ~2110 lines (slight reduction from inlined closure extraction reducing duplication). Largest file is `result_handling.rs` at ~500 lines, well within the 500-800 target. The `mod.rs` shell at ~250 lines is manageable and serves as a readable entry point.

## Detailed file breakdown

### `mod.rs` (~250 lines)

**What moves here:**
- All `mod` and `pub use` declarations
- Constants: `WORKER_POOL_ID`, `MERGE_WORKER_ID`, `WORKER_COMMAND_HISTORY_LIMIT`, `COPY_SHORTCUT_KEY`, `IDLE_WORKER_POLL_DELAY_MS`, `IDLE_WORKER_POLL_ATTEMPTS`
- Internal types: `enum PoolResultMessage`, `enum PoolStreamEvent`
- `pub fn run_worker_pool_fsm` -- but refactored to be a thin orchestration shell:
  - Initialization (clear interrupt, reset scroll, log start, create worker rows)
  - Outer `while completed < target` loop calling into extracted functions from `scheduling.rs` and `result_handling.rs`
  - Post-loop shutdown screen logic

**Why:** This file is the entry point. Keeping the top-level FSM structure here makes it easy to understand the overall flow at a glance, while the details of each concern live in sub-files. The `PoolResultMessage` and `PoolStreamEvent` enums stay here because they are the "lingua franca" types used by multiple sub-files -- keeping them in `mod.rs` avoids circular dependencies.

**Visibility:** `PoolResultMessage` and `PoolStreamEvent` become `pub(super)` so sub-files can reference them.

### `event_handling.rs` (~180 lines)

**What moves here:**
- `fn apply_pool_stream_event` (currently lines 1139-1202)
- `fn drain_pool_events` (currently lines 1204-1218)
- `fn is_non_regressive_state_transition` (currently lines 1506-1520)
- `fn worker_state_rank` (currently lines 1522-1537)
- `fn normalize_worker_state_for_transition` (currently lines 1539-1571)

**Why:** These functions form a cohesive "event processing and state transition validation" concern. They are called from the inner event loop in `result_handling.rs` and from `apply_pool_stream_event` itself. Grouping them makes it clear where to look when debugging event-related TUI issues or adding new worker states.

**Imports needed from parent:** `PoolStreamEvent`, `WorkerRow` (from `crate::tui`), `append_worker_command` (from `scheduling.rs`), `format_state_label` (from `crate::tui`).

**Visibility:** All functions become `pub(super)` so `mod.rs` and `result_handling.rs` can call them.

### `hotkeys.rs` (~170 lines)

**What moves here:**
- `struct HotkeyState<'a>` (currently lines 96-105)
- `fn handle_hotkeys` (currently lines 1296-1436)
- `fn hotkey_action` (currently lines 1577-1579)
- `fn is_copy_shortcut_key` (currently lines 1573-1575)
- `fn wait_for_quit` (currently lines 1220-1294)

**Why:** Hotkey handling is a self-contained interactive concern -- it reads keyboard input, dispatches actions, and renders the appropriate screen. It has no dependencies on the scheduling or result-handling logic. When adding new hotkeys or changing TUI interaction, this is the only file to touch.

**Imports needed from parent:** `WORKER_POOL_ID`, `COPY_SHORTCUT_KEY` constants, `dashboard_snapshot` and `render` from `dashboard.rs`, `quality_report_path` from `dashboard.rs`.

**Visibility:** `HotkeyState` and `handle_hotkeys` become `pub(super)`. `wait_for_quit` becomes `pub(super)`.

### `dashboard.rs` (~120 lines)

**What moves here:**
- `struct DashboardSnapshot` (currently lines 1581-1585)
- `fn dashboard_snapshot` (currently lines 1587-1646)
- `fn render` (currently lines 1648-1673)
- `fn short_task_id` (currently lines 1675-1677)
- `fn worker_failure_prompt` (currently lines 1679-1689)
- `fn quality_report_path` (currently lines 1691-1698)

**Why:** These are all "what to show on screen" functions -- they query state and format it for display. None of them mutate worker pool state. This clean read-only boundary makes this the smallest and simplest file, and the most likely to be touched independently (e.g., when changing dashboard layout or adding new stats).

**Imports needed from parent:** `WORKER_POOL_ID` constant, `BacklogStore`, `QueueStats`, `BacklogView`, `WorkerRow`.

**Visibility:** `DashboardSnapshot` becomes `pub(super)`. All functions become `pub(super)`.

### `scheduling.rs` (~200 lines)

**What moves here:**
- The logic currently inside the `claim_tasks_for_available_workers` closure (lines 182-231), extracted into a standalone function
- The logic currently inside the `mark_merge_worker_busy` closure (lines 233-251), extracted into a standalone function
- The logic currently inside the `maybe_start_merge` closure (lines 271-337), extracted into a standalone function
- `fn set_worker_idle` (currently lines 1470-1481)
- `fn append_worker_command` (currently lines 1445-1453)
- `fn execution_task_packet` (currently lines 1455-1468)
- `fn refresh_worker_heartbeats` (currently lines 1483-1492)

**Why:** These functions handle "which worker gets which task" -- the scheduling and claim logic. They share a common pattern of mutating worker rows and interacting with the backlog store. The three closures currently capture many local variables; extracting them into functions with explicit parameter structs improves testability and readability.

**Closure extraction approach:** The closures capture `parallelism`, `target`, `run_started_at_ms`, `store`, `cfg`, `terminal`, and various mutable references. They should become functions that take a parameter struct (or explicit args), e.g.:

```rust
pub(super) struct ClaimContext<'a> {
    pub(super) parallelism: usize,
    pub(super) store: &'a BacklogStore,
    pub(super) cfg: &'a AppConfig,
    pub(super) terminal: &'a dyn Terminal,
    pub(super) run_started_at_ms: i64,
    pub(super) hb: u64,
    pub(super) lt: u64,
}
```

**Imports needed from parent:** `WORKER_POOL_ID`, `MERGE_WORKER_ID`, `WORKER_COMMAND_HISTORY_LIMIT` constants, `WorkerRow`, `BacklogStore`, `AppConfig`.

**Visibility:** All functions become `pub(super)`.

### `result_handling.rs` (~500 lines)

**What moves here:**
- `fn handle_doing_result` -- extracted from the `Ok(PoolResultMessage::DoingResult { .. })` match arm (currently lines 548-868). This is the largest chunk: it handles errors, `HandoffToMerge`, `Completed`, and `Failed` outcomes, updates worker TUI state, logs events, and re-claims the next task.
- `fn handle_merge_result` -- extracted from the `Ok(PoolResultMessage::MergeResult { .. })` match arm (currently lines 870-994).
- `fn spawn_doing_workers` -- extracted from the doing worker spawn block (currently lines 408-460) and the re-spawn block (currently lines 807-857). These are nearly identical code blocks that should become a single function called twice.
- `fn spawn_merge_worker` -- extracted from the merge worker thread spawn (currently lines 462-508).
- `fn run_inner_event_loop` -- the `while active_doing > 0 || active_merging > 0` loop body (currently lines 514-1054), refactored to call `handle_doing_result`, `handle_merge_result`, and `drain_pool_events`.

**Why:** This is the operational heart of the worker pool -- what happens when workers finish tasks. It is the most complex and most frequently modified code. Isolating it makes the inner event loop's structure visible without scrolling through 500+ lines of match arms. The doing-worker spawn logic appears twice (initial spawn and re-claim spawn) with near-identical code; extracting it eliminates duplication.

**Imports needed from parent:** `PoolResultMessage`, `PoolStreamEvent`, constants, most `crate::*` imports.

**Visibility:** All functions become `pub(super)`.

### `util.rs` (~30 lines)

**What moves here:**
- `fn now_unix_millis` (currently lines 1438-1443)
- `fn now_hhmmss` (currently lines 1494-1504)
- `struct ShutdownSummary` + `impl ShutdownSummary::format_message` (currently lines 66-94)

**Why:** These are pure utility functions and a simple formatting struct with no dependencies on pool internals. They are used across multiple sub-files. Keeping them in a small, obvious utility file avoids the "where does this live?" question.

**Visibility:** All become `pub(super)`.

### `tests.rs` (~660 lines)

**What moves here:**
- The entire `#[cfg(test)] mod tests` block (currently lines 1700-2354)
- Test helpers: `seed_task`, `test_scope`, `write_file`

**Why:** At 655 lines, the test module is substantial. Moving it to its own file follows the pattern used in large Rust projects and keeps `mod.rs` focused on production code. Tests can still access all `pub(super)` items because `tests.rs` is declared as `#[cfg(test)] mod tests;` in `mod.rs`.

**Note:** Some tests exercise functions that will move to sub-files (e.g., `is_non_regressive_state_transition` in `event_handling.rs`, `apply_pool_stream_event` in `event_handling.rs`, `hotkey_action` in `hotkeys.rs`). The tests import from `super::*` which will resolve correctly since `mod.rs` re-exports or the test file can import from sibling modules via `super::event_handling::*` etc.

## Migration steps

### Phase 1: Create the directory and mod.rs

1. `mkdir src/worker_pool`
2. `mv src/worker_pool.rs src/worker_pool/mod.rs` (preserves git history)
3. Run `cargo check -p gardener` -- should pass identically since `worker_pool/mod.rs` resolves the same as `worker_pool.rs`

### Phase 2: Extract leaf modules (no internal deps)

Extract in dependency order, running `cargo check` and `cargo test -p gardener` between each step:

1. **`util.rs`** -- pure functions, no internal deps
   - Move `now_unix_millis`, `now_hhmmss`, `ShutdownSummary` + impl
   - Add `mod util;` to `mod.rs`
   - Replace direct calls in `mod.rs` with `util::now_unix_millis()` etc., or add `use util::*;` at top of `mod.rs`

2. **`event_handling.rs`** -- depends only on `crate::tui` and `util`
   - Move `apply_pool_stream_event`, `drain_pool_events`, `is_non_regressive_state_transition`, `worker_state_rank`, `normalize_worker_state_for_transition`
   - Add `mod event_handling;` to `mod.rs`
   - The `PoolStreamEvent` enum stays in `mod.rs` and is referenced as `super::PoolStreamEvent` from `event_handling.rs`

3. **`dashboard.rs`** -- depends on `crate::tui`, `crate::backlog_store`, `util`
   - Move `DashboardSnapshot`, `dashboard_snapshot`, `render`, `short_task_id`, `worker_failure_prompt`, `quality_report_path`
   - Add `mod dashboard;` to `mod.rs`

4. **`hotkeys.rs`** -- depends on `dashboard`, `crate::hotkeys`, `crate::runtime`
   - Move `HotkeyState`, `handle_hotkeys`, `wait_for_quit`, `hotkey_action`, `is_copy_shortcut_key`
   - Add `mod hotkeys;` to `mod.rs`

5. **`scheduling.rs`** -- depends on `dashboard` (for `render`, `dashboard_snapshot`), `util`
   - Extract the three closures into standalone functions with explicit parameters
   - Move `set_worker_idle`, `append_worker_command`, `execution_task_packet`, `refresh_worker_heartbeats`
   - Add `mod scheduling;` to `mod.rs`
   - **This is the trickiest step** -- see Risk Assessment below

6. **`result_handling.rs`** -- depends on `scheduling`, `event_handling`, `dashboard`, `util`
   - Extract `handle_doing_result`, `handle_merge_result` from match arms
   - Extract `spawn_doing_workers`, `spawn_merge_worker`
   - Restructure the inner event loop into `run_inner_event_loop`
   - Add `mod result_handling;` to `mod.rs`
   - **This is the second trickiest step** -- the inner loop has deep coupling to mutable state

7. **`tests.rs`** -- depends on everything via `super::*`
   - Move the entire `#[cfg(test)] mod tests` block
   - Add `#[cfg(test)] mod tests;` to `mod.rs`
   - Update test imports: `use super::*` should still work if `mod.rs` has appropriate `pub(super)` re-exports or `use` statements

### Phase 3: Cleanup

1. Verify `mod.rs` is under ~250 lines and reads as a clear top-level orchestrator
2. Run full verification:
   ```bash
   cargo check -p gardener
   cargo test -p gardener
   cargo clippy -p gardener
   ```
3. Confirm `pub fn run_worker_pool_fsm` is still the sole public export

## Public API preservation

The `mod.rs` file needs exactly one re-export:

```rust
mod dashboard;
mod event_handling;
mod hotkeys;
mod result_handling;
mod scheduling;
mod util;

#[cfg(test)]
mod tests;

// -- sole public export, unchanged signature --
pub fn run_worker_pool_fsm(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    store: &BacklogStore,
    terminal: &dyn Terminal,
    target: usize,
    task_override: Option<&str>,
) -> Result<usize, GardenerError> {
    // ... orchestration body ...
}
```

No changes to `lib.rs` or any other consumer.

## Risk assessment

### High risk: Closure extraction in `scheduling.rs`

The three closures (`claim_tasks_for_available_workers`, `mark_merge_worker_busy`, `maybe_start_merge`) capture a large number of local variables by mutable reference. Converting them to standalone functions requires:
- Defining a parameter struct (or using many explicit arguments) to replace captures
- Ensuring mutable borrows don't conflict (the closures currently rely on the borrow checker seeing them as single-scope borrows)
- The `claim_tasks_for_available_workers` closure captures `parallelism`, `target`, `run_started_at_ms`, `store`, `cfg`, `terminal`, `hb`, `lt` -- and takes `workers`, `claimed`, `completed`, `last_worker_state_line`, `last_activity_pulse` as explicit parameters

**Mitigation:** Extract closures one at a time. Consider a `PoolContext` struct that holds the immutable shared state, passed by reference:
```rust
pub(super) struct PoolContext<'a> {
    pub(super) parallelism: usize,
    pub(super) target: usize,
    pub(super) run_started_at_ms: i64,
    pub(super) store: &'a BacklogStore,
    pub(super) cfg: &'a AppConfig,
    pub(super) terminal: &'a dyn Terminal,
    pub(super) hb: u64,
    pub(super) lt: u64,
}
```

### High risk: Inner event loop extraction in `result_handling.rs`

The `while active_doing > 0 || active_merging > 0` loop body (lines 514-1054) references many mutable local variables: `active_doing`, `active_merging`, `workers`, `last_activity_pulse`, `event_sequence`, `merge_tx`, `quit_requested`, `quit_requested_at`, `last_shutdown_log`, `last_dashboard_refresh`, `last_render_completed`, `last_render_heartbeat`, `shutdown_error`, `completed`, `merged`, `failed`, `last_worker_state_line`.

**Mitigation:** Define an `EpochState` struct to bundle mutable loop state:
```rust
pub(super) struct EpochState {
    pub(super) active_doing: usize,
    pub(super) active_merging: usize,
    pub(super) completed: usize,
    pub(super) merged: usize,
    pub(super) failed: usize,
    pub(super) quit_requested: bool,
    pub(super) quit_requested_at: Option<Instant>,
    pub(super) shutdown_error: Option<(String, String, String)>,
    pub(super) last_worker_state_line: usize,
    pub(super) event_sequence: usize,
    pub(super) last_shutdown_log: Instant,
    pub(super) last_dashboard_refresh: Instant,
    pub(super) last_render_completed: Option<Instant>,
    pub(super) last_render_heartbeat: Instant,
}
```

This struct is passed by `&mut` to `handle_doing_result` and `handle_merge_result`, keeping the inner loop in `result_handling.rs` readable.

### Medium risk: Doing-worker re-spawn inside `std::thread::scope`

The re-claim and re-spawn logic (lines 760-857) happens inside the `std::thread::scope` closure, which requires the `scope_guard` handle to spawn new threads. This means `handle_doing_result` either needs to receive the `scope_guard` as a parameter (which requires careful lifetime annotation), or the re-spawn logic stays inline in `mod.rs` while the result-processing logic lives in `result_handling.rs`.

**Mitigation:** The cleanest approach is:
1. `handle_doing_result` returns a `DoingAction` enum indicating what to do next (re-claim, idle, shutdown, etc.)
2. The `mod.rs` inner loop matches on the action and performs the spawn inline (since it has the `scope_guard`)
3. This avoids passing `scope_guard` across module boundaries while still extracting the complex match logic

```rust
pub(super) enum DoingAction {
    Idle,
    ReclaimAndSpawn { idx: usize, task: BacklogTask },
    Shutdown,
    SignalMerge,
}
```

### Low risk: Test imports after split

Tests currently use `use super::{...}` to import private items. After the split, items from sub-files need to be visible to the test module. Two approaches:
1. Add `pub(super) use` re-exports in `mod.rs` for items tests need
2. Have `tests.rs` import directly from sibling modules: `use super::event_handling::is_non_regressive_state_transition;`

Option 2 is cleaner and makes test dependencies explicit.

### Low risk: Duplicate doing-worker spawn code

Lines 408-460 (initial spawn) and lines 807-857 (re-claim spawn) are nearly identical. Extracting a `spawn_doing_worker` function that takes `scope_guard`, `tx`, `event_tx`, and task details eliminates this duplication. The function returns the thread `ScopedJoinHandle` which the caller can ignore (as it currently does).

## Verification

After each extraction step:
```bash
cargo check -p gardener         # compilation
cargo test -p gardener           # all existing tests pass
cargo clippy -p gardener         # no new warnings
```

No behavioral changes -- purely a file reorganization with internal visibility adjustments.
