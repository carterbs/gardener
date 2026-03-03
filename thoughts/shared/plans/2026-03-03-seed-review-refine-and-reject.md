# Seed Review: Refine Tasks & Discard with Reason

## Overview

Extend the interactive backlog seeding review wizard with two new capabilities:

1. **Refine**: Press `r` to provide free-form feedback on a task. After all tasks are reviewed, refined tasks go back to the seeding agent with the user's feedback for a new round. Loop until every task is either kept or discarded.

2. **Discard with reason**: Press `d` to discard a task, then optionally type why. Persist rejected tasks with their reasons in a dedicated table. Feed them into the seeding prompt on future runs to prevent re-suggesting the same work.

## Current State

- `run_seed_review_wizard` (`tui.rs:1866-1898`) returns `Vec<bool>` — keep or discard, no text input
- `SeedReviewState` (`tui.rs:2397-2435`) tracks `current`, `total`, `kept: Vec<bool>`
- `draw_seed_review_frame` (`tui.rs:1745-1864`) renders task card with `[k] Keep  [d] Discard  [q] Finish` footer
- `run_interactive_seeding` (`startup.rs:910-1038`) calls the wizard once, inserts kept tasks, drops discarded
- No persistence of rejected tasks anywhere
- Seeding prompt (`seeding.rs:57-84`) has no "previously rejected" section
- DB schema has 7 statuses, no `rejected` status, no rejection reason column

## Design Decisions

### Separate `rejected_seed_tasks` table (not a new backlog status)

Adding `rejected` to the backlog `status` CHECK constraint would require a table-rebuild migration and pollute the worker state machine (which only cares about ready→leased→in_progress→complete transitions). A dedicated table is cleaner:

- No interference with the task FSM
- Simple schema: just the seed fields + rejection reason + timestamp
- Easy to query for the seeding prompt
- Easy to prune later if needed

### One agent call per refinement round (batched)

Rather than calling the agent once per refined task, batch all refined tasks into a single prompt. The agent returns revised versions of all tasks in one JSON envelope. This keeps the round-trip count low.

### Text input mode in TUI

Both `d` (discard reason) and `r` (refine feedback) enter a text input mode. The input is rendered inline below the task card. Enter submits, Esc cancels (reverts to the task without advancing). For discard, the reason is optional (Enter on empty = discard with no reason). For refine, feedback is required (Enter on empty = no-op, stays in input mode).

---

## Implementation

### Phase 1: New `ReviewDecision` type and updated wizard return

**Goal**: Replace `Vec<bool>` with a richer decision type that can carry text.

**Changes**:

- `tui.rs` — Add `ReviewDecision` enum (public, near `SeedReviewState`):
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum ReviewDecision {
      Keep,
      Discard(Option<String>),  // optional rejection reason
      Refine(String),           // required feedback text
  }
  ```

- `tui.rs:2397-2405` — Rewrite `SeedReviewState`:
  ```rust
  struct SeedReviewState {
      current: usize,
      total: usize,
      decisions: Vec<Option<ReviewDecision>>,  // None = not yet reviewed
      input_mode: Option<InputMode>,           // None = normal key mode
      input_buffer: String,                    // text being typed
  }

  enum InputMode { DiscardReason, RefineFeedback }
  ```

- `tui.rs:1866-1898` — Update `run_seed_review_wizard` signature:
  ```rust
  pub fn run_seed_review_wizard(tasks: &[SeedTask]) -> Result<Vec<ReviewDecision>, GardenerError>
  ```
  On return, any `None` decisions (from `q`/Esc early exit) become `Discard(None)`.

- `startup.rs:974-995` — Update call site to work with `Vec<ReviewDecision>` instead of `Vec<bool>`. (Will be further modified in Phase 4 for the refinement loop.)

**Success criteria**: Build + test green. Wizard returns `Vec<ReviewDecision>`. Existing `k`/`d`/`q` behavior preserved (discard reason is `None` for now since text input isn't wired yet).

---

### Phase 2: Text input mode in the TUI

**Goal**: When the user presses `d` or `r`, show an inline text input field. Enter submits, Esc cancels.

**Changes**:

- `tui.rs:2414-2434` — Rewrite `handle_key`:
  - **Normal mode** (no `input_mode`):
    - `k`/`K` → `decisions[current] = Some(Keep)`, advance, `ReviewAction::Continue`
    - `d`/`D` → enter `InputMode::DiscardReason`, clear `input_buffer`, `ReviewAction::Continue` (do NOT advance yet)
    - `r`/`R` → enter `InputMode::RefineFeedback`, clear `input_buffer`, `ReviewAction::Continue`
    - `q`/`Q`/`Esc` → `ReviewAction::Finish`
  - **Input mode** (`input_mode` is `Some`):
    - `Enter` →
      - If `DiscardReason`: `decisions[current] = Some(Discard(reason_or_none))`, advance, exit input mode
      - If `RefineFeedback` and buffer is empty: no-op (stay in input mode, feedback is required)
      - If `RefineFeedback` and buffer has text: `decisions[current] = Some(Refine(buffer.clone()))`, advance, exit input mode
    - `Esc` → exit input mode, do NOT advance (go back to normal mode on the same task)
    - `Backspace` → pop last char from `input_buffer`
    - `Char(c)` → push `c` to `input_buffer`
    - Everything else → no-op

- `tui.rs:1745-1864` — Update `draw_seed_review_frame` signature to accept `input_mode: Option<&InputMode>` and `input_text: &str`:
  - When `input_mode` is `None`: render the normal footer `[k] Keep  [d] Discard  [r] Refine  [q] Finish`
  - When `input_mode` is `Some(DiscardReason)`: replace footer area with:
    ```
    Why discard? (optional — Enter to skip, Esc to cancel)
    > {input_text}█
    ```
  - When `input_mode` is `Some(RefineFeedback)`: replace footer area with:
    ```
    How should this task change? (Enter to submit, Esc to cancel)
    > {input_text}█
    ```
  - The footer area height may need to grow from `Length(2)` to `Length(3)` when in input mode (label line + input line + border). Use a conditional layout.

- `tui.rs:1866-1898` — Update the draw call in the event loop to pass `state.input_mode.as_ref()` and `&state.input_buffer`.

**Success criteria**: Build + test green. Pressing `d` shows discard reason prompt, Enter submits (empty = no reason). Pressing `r` shows refine prompt, Enter submits (only with text). Esc cancels both and returns to normal mode.

---

### Phase 3: Discard persistence — `rejected_seed_tasks` table

**Goal**: Create a table for rejected tasks and write discarded tasks to it.

**Changes**:

- `migrations/0005_rejected_seeds.sql` — New migration:
  ```sql
  CREATE TABLE IF NOT EXISTS rejected_seed_tasks (
      id              TEXT PRIMARY KEY,
      title           TEXT NOT NULL,
      details         TEXT NOT NULL,
      rationale       TEXT NOT NULL DEFAULT '',
      domain          TEXT NOT NULL DEFAULT 'infrastructure',
      priority        TEXT NOT NULL DEFAULT 'P1',
      rejection_reason TEXT NOT NULL DEFAULT '',
      rejected_at     INTEGER NOT NULL
  );
  ```
  The `id` is a hash of the title (or title+domain) to deduplicate across runs.

- `backlog_store.rs` — Add methods:
  - `insert_rejected_seed(task: &SeedTask, reason: Option<&str>)` — INSERT OR REPLACE into `rejected_seed_tasks`
  - `list_rejected_seeds() -> Vec<RejectedSeed>` — SELECT all rows, ordered by `rejected_at` DESC
  - Add `RejectedSeed` struct:
    ```rust
    pub struct RejectedSeed {
        pub title: String,
        pub details: String,
        pub rejection_reason: String,
        pub domain: String,
    }
    ```

- `backlog_store.rs:1537-1583` — Update migration runner to apply `0005_rejected_seeds.sql`.

- `startup.rs` — After the review wizard, for each `Discard(reason)` decision:
  ```rust
  store.insert_rejected_seed(&recommendations[index], reason.as_deref())?;
  ```

**Success criteria**: Build + test green. Migration applies cleanly on existing DBs. Discarded tasks with reasons are persisted. `list_rejected_seeds()` returns them.

---

### Phase 4: Feed rejected tasks into seeding prompt

**Goal**: The seeding agent sees previously rejected tasks so it doesn't re-suggest them.

**Changes**:

- `seeding.rs:16-27` — Add field to `SeedPromptContext`:
  ```rust
  pub rejected_tasks: String,
  ```

- `seeding.rs:285-314` (`build_seed_prompt_context`) — Populate `rejected_tasks`:
  - Call `store.list_rejected_seeds()`
  - Format as:
    ```
    - "Task title" (domain) — rejected because: "reason"
    ```
  - If no reason was given: `— rejected (no reason given)`
  - Cap at ~20 entries to avoid bloating the prompt

- `seeding.rs:57-84` (`build_seed_dry_run_prompt_v1`) — Add a new section after "Existing active backlog snapshot":
  ```
  \nPreviously rejected seed tasks — do NOT suggest these again\n
  {rejected_tasks}
  ```
  Only include the section if `rejected_tasks` is non-empty.

- `seeding.rs:86-129` (`render_seed_prompt_template` / v2 path) — Add `{REJECTED_TASKS}` token and matching substitution for the auto-seed template path as well, so both paths stay in sync.

**Success criteria**: Build + test green. Seeding prompt includes rejected tasks section. Agent output no longer re-suggests previously rejected work (verified manually).

---

### Phase 5: Refinement loop in `startup.rs`

**Goal**: After the initial review, tasks marked `Refine` go back to the agent. Loop until all tasks are kept or discarded.

**Changes**:

- `seeding.rs` — Add `build_seed_refine_prompt(tasks_with_feedback: &[(SeedTask, String)], context: &SeedPromptContext) -> String`:
  ```
  You are refining previously suggested backlog tasks based on user feedback.

  For each task below, produce a revised version that addresses the user's feedback.
  Keep the same JSON envelope format. Return exactly {N} tasks in the same order.

  Task 1:
  Title: {title}
  Details: {details}
  Rationale: {rationale}

  User feedback: "{feedback}"

  ---

  Task 2:
  ...

  [same quality_risks, existing_backlog, rejected_tasks context as normal seeding]
  ```

- `seed_runner.rs` — Add `run_seed_refine_with_events(prompt, backend, model, event_tx) -> Result<Vec<SeedTask>>`:
  - Same structure as `run_legacy_seed_runner_v1_with_events` but uses the refinement prompt
  - Same JSON parsing, same schema validation
  - Same `max_turns: Some(12)`

- `startup.rs` — Add `run_seed_refinement_with_heartbeat(...)`:
  - Same heartbeat wrapper pattern as `run_seed_recommendations_with_heartbeat`
  - Calls `run_seed_refine_with_events` in a thread
  - Shows the seeding TUI screen during agent work

- `startup.rs:949-1004` — Rewrite the review-mode block as a loop:
  ```rust
  let mut pending_tasks: Vec<SeedTask> = run_seed_recommendations_with_heartbeat(...)?;

  loop {
      if pending_tasks.is_empty() { break; }

      runtime.terminal.close_ui();
      let decisions = tui::run_seed_review_wizard(&pending_tasks)?;

      // Process decisions
      let mut to_refine: Vec<(SeedTask, String)> = Vec::new();
      for (i, decision) in decisions.into_iter().enumerate() {
          match decision {
              ReviewDecision::Keep => {
                  // insert into backlog (existing logic)
                  store.upsert_task(new_task_from_seed(&pending_tasks[i]))?;
                  seeded += 1;
              }
              ReviewDecision::Discard(reason) => {
                  store.insert_rejected_seed(&pending_tasks[i], reason.as_deref())?;
              }
              ReviewDecision::Refine(feedback) => {
                  to_refine.push((pending_tasks[i].clone(), feedback));
              }
          }
      }

      if to_refine.is_empty() { break; }

      // Re-seed with refinement feedback
      pending_tasks = run_seed_refinement_with_heartbeat(
          runtime, &to_refine, &scope, &cfg, ...
      )?;
      // Loop back to show wizard with revised tasks
  }
  ```

**Success criteria**: Build + test green. Refined tasks go back to agent, return as revised tasks, wizard shows them again. Loop exits when no more refinements. Kept tasks inserted, discarded tasks persisted with reasons.

---

### Phase 6: Update `draw_seed_review_frame` footer and header for refinement rounds

**Goal**: Visual polish — make it clear when you're in a refinement round and what the new keys do.

**Changes**:

- `tui.rs:1745` — Update `draw_seed_review_frame` signature to accept `round: usize` (0-indexed):
  - Round 0 header: `"review backlog  (3/10)"`
  - Round 1+ header: `"review backlog  round 2  (1/3)"`

- `tui.rs:1838-1863` — Update footer in normal mode:
  ```
  [k] Keep    [d] Discard    [r] Refine    [q] Discard remaining & finish
  ```
  Colors: `[k]` green, `[d]` red, `[r]` yellow/amber `Rgb(245,196,95)`, `[q]` gray

- `startup.rs` refinement loop — Pass `round` counter to the wizard:
  ```rust
  pub fn run_seed_review_wizard(tasks: &[SeedTask], round: usize) -> Result<Vec<ReviewDecision>, GardenerError>
  ```

**Success criteria**: Build + test green. Round number visible in header on refinement rounds. All four keys shown in footer.

---

## Testing Strategy

### Unit tests
- `ReviewDecision` serialization/deserialization round-trip (if we ever serialize it)
- `SeedReviewState` with `InputMode` transitions:
  - `d` → `DiscardReason` mode → Enter with empty → `Discard(None)`
  - `d` → `DiscardReason` mode → type text → Enter → `Discard(Some("reason"))`
  - `r` → `RefineFeedback` mode → Enter on empty → stays in mode
  - `r` → `RefineFeedback` mode → type text → Enter → `Refine("text")`
  - Esc from input mode → back to normal mode, same task
- `build_seed_refine_prompt` output format validation
- `rejected_seed_tasks` migration on fresh and existing DBs
- `insert_rejected_seed` + `list_rejected_seeds` round-trip
- `list_rejected_seeds` caps at 20 entries

### Integration tests
- Full refinement loop with mock agent returning revised tasks
- Discard persistence survives across seeding runs (rejected tasks appear in prompt)

### Manual verification
- Full flow: seed → review → refine some → agent revises → review again → keep/discard all
- Discard with reason → run seeding again → verify rejected tasks appear in agent prompt
- Text input rendering (cursor, backspace, long text wrapping)
- Esc from input mode returns to task without advancing
- `q` during review discards remaining and exits loop

## File Change Summary

| File | Changes |
|---|---|
| `tui.rs` | `ReviewDecision` enum, `InputMode` enum, rewrite `SeedReviewState`, rewrite `handle_key`, update `draw_seed_review_frame` (footer, input area, round), update `run_seed_review_wizard` signature/return type |
| `startup.rs` | Refinement loop in `run_interactive_seeding`, call `insert_rejected_seed` for discards, pass round counter |
| `seeding.rs` | `rejected_tasks` field on `SeedPromptContext`, `build_seed_refine_prompt`, inject rejected tasks section into both v1 and v2 prompts |
| `seed_runner.rs` | `run_seed_refine_with_events` (or reuse existing runner with different prompt) |
| `backlog_store.rs` | `RejectedSeed` struct, `insert_rejected_seed`, `list_rejected_seeds`, migration runner update |
| `migrations/0005_rejected_seeds.sql` | New table `rejected_seed_tasks` |

## References

- `tui.rs:1745-1864` — `draw_seed_review_frame` (to modify)
- `tui.rs:1866-1898` — `run_seed_review_wizard` (to modify)
- `tui.rs:2397-2435` — `SeedReviewState` (to rewrite)
- `startup.rs:910-1038` — `run_interactive_seeding` (to add refinement loop)
- `startup.rs:774-878` — `run_seed_recommendations_with_heartbeat` (pattern for refinement heartbeat)
- `seeding.rs:57-84` — `build_seed_dry_run_prompt_v1` (to add rejected tasks section)
- `seeding.rs:285-314` — `build_seed_prompt_context` (to add rejected_tasks field)
- `seed_runner.rs:57-205` — `run_legacy_seed_runner_v1_with_events` (pattern for refinement runner)
- `backlog_store.rs:1537-1583` — migration runner (to add migration 0005)
