# Interactive Backlog Seeding with Approval Flow

## Overview

Replace the current silent/fallback seeding with an interactive system: on startup, if the backlog is empty, run seeding with a live TUI screen showing agent activity, then either auto-insert tasks or present them one-by-one for user approval. Add a universal shutdown summary for all exit paths.

## Current State Analysis

### Seeding flow (`startup.rs:401-555`)
- `should_seed_backlog` gate at line 693: seeds when `run_seeding && !test_mode && (count == 0 || force)`
- Write path (v2 direct): agent calls `backlog-db.sh add` directly — no structured output captured
- Dry-run path (legacy v1): agent returns JSON `SeedPayload { tasks: Vec<SeedTask> }` with `title`, `details`, `rationale`, `domain`, `priority`
- Fallback path (`startup.rs:1087-1156`): when agent seeds 0 tasks, generates `QualityGap` "Improve X from F to B" tasks from quality doc — **this is the garbage**

### Seeding TUI display (`startup.rs:702-788`, `lib.rs:488`)
- During seeding, `run_seed_with_heartbeat` relays `AgentEvent` summaries to `draw_boot_stage(runtime, "BACKLOG_SYNC", detail)`, which renders a single synthetic "sys" `WorkerRow` in the dashboard. No dedicated screen.

### Setup wizard (`tui.rs:2015-2213`)
- 4-step blocking wizard: Parallelism → Validation → Docs → Notes
- Returns `RepoHealthWizardAnswers` with `preferred_parallelism`, `validation_command`, `external_docs_accessible`, `additional_context`
- Called from `triage_interview.rs:93`, answers flow into `UserValidated` in the profile

### Profile persistence (`repo_intelligence.rs:12-18`)
- `RepoIntelligenceProfile` → serialized as TOML to `~/.gardener/<repo>/repo-intelligence.toml`
- `UserValidated` struct holds wizard answers (line 39-52)

### Shutdown (`worker_pool.rs:1033-1064`)
- On `quit_requested` (q/Ctrl+C): `return Ok(completed)` — **skips shutdown screen entirely**
- On normal completion: shows "All Tasks Complete" or "No More Work" with basic count
- No merge count, no rich summary

## Desired End State

1. No fallback seeding — if the agent produces nothing, the backlog stays empty
2. Onboarding asks: approve tasks or auto-seed?
3. On startup with empty backlog: new "Seeding your backlog" TUI screen with live command stream
4. Approve path: task-by-task review (1/N, 2/N) with Keep/Discard
5. Auto-seed path: insert directly, show activity, proceed to dashboard
6. Universal shutdown summary with merge/completion counts for ALL exit paths (including q/Ctrl+C)

## What We're NOT Doing

- Changing the seeding prompt or agent behavior (beyond requiring structured output for approval path)
- Adding mid-run re-seeding
- Changing the triage discovery or agent detection flows
- Modifying the worker FSM or merge loop

## Key Discoveries

1. **Two seeding modes already exist**: write (v2 direct, no JSON) and dry-run (v1 legacy, returns `SeedTask` JSON). The approval flow needs structured output, so we should use the dry-run/JSON path for the approval flow and the direct-write path for auto-seed.

2. **`SeedTask` already has `rationale`** (`seed_runner.rs:18`). The schema requires `rationale` with `minLength: 10`. We need to verify the prompt explicitly asks for "why this makes agents more effective" — current prompt says "rationale should state the immediate quality signal and why now" which is close but should be tweaked.

3. **The wizard is a standalone blocking loop** (`tui.rs:2015`) that owns its own terminal. Adding a step is straightforward — increment step count, add a new match arm.

4. **`UserValidated` in the profile** needs a new field for the seeding preference. This is TOML-serialized with `serde`, so adding `#[serde(default)]` keeps backward compat.

5. **The quit path (`worker_pool.rs:1033-1034`) returns immediately** — it never hits the shutdown screen. This needs to change.

---

## Implementation Approach

### Phase 1: Remove fallback seeding

**Goal**: Delete the garbage task generation path.

**Changes**:
- `startup.rs`: Delete `fallback_seed_tasks` (lines 1087-1156), `fallback_from_quality_doc` (lines 1158-1185), and the `seed_generation` helper (lines 1072-1085)
- `startup.rs:522-554`: Remove the `else` branch that calls `fallback_seed_tasks` when `agent_seeded == 0`. Replace with a log message and continue.
- `startup.rs:431`: Remove `fallback_target` computation
- Delete associated tests for `fallback_seed_tasks` and `fallback_from_quality_doc`

**Success criteria**:
- `cargo test` passes
- No `quality_gap` tasks generated on empty agent seeding
- Seeding with 0 agent results simply logs and moves on

**Confirmation gate**: Build + test green

---

### Phase 2: Add seeding preference to onboarding wizard

**Goal**: New wizard step asking approve vs auto-seed, with explanation.

**Changes**:

- `tui.rs:2007-2013` — Add `backlog_approval: bool` to `RepoHealthWizardAnswers`
- `tui.rs:85` — Change `WIZARD_STEP_LABELS` from 4 to 5 items: `["Parallelism", "Validation", "Docs", "Backlog", "Notes"]`
- `tui.rs:2025-2213` — Add step 3 (shifting Notes to step 4):
  - Label: "Backlog seeding"
  - Help text: "Gardener seeds a backlog of tasks that make your repo more hospitable to coding agents. Review each task before it's added, or auto-seed?"
  - Input: `a` for auto-seed, `r` for review (default: review)
  - Display: `> auto-seed` or `> review tasks`
- `triage_interview.rs:9-18` — Add `backlog_approval: bool` to `InterviewResult`
- `triage_interview.rs:93-108` — Pass wizard `backlog_approval` through to `InterviewResult`
- `repo_intelligence.rs:39-52` — Add `#[serde(default)] pub backlog_approval: bool` to `UserValidated` (default = false = auto-seed for backward compat with existing profiles)
- `triage.rs:344-350` — Wire `interview.backlog_approval` into `profile.user_validated.backlog_approval`

**Success criteria**:
- Wizard shows 5 steps with Backlog between Docs and Notes
- Choice persists in TOML profile
- Existing profiles without the field default to `false` (auto-seed)

**Confirmation gate**: Build + test green, manually verify wizard renders new step

---

### Phase 3: New "Seeding your backlog" TUI screen

**Goal**: A dedicated seeding screen that streams agent commands, used by both auto-seed and approval paths.

**Changes**:

- `tui.rs` — Add `draw_seeding_frame` and `draw_seeding_live`:
  - Header: "GARDENER  seeding your backlog"
  - Body: scrollable list of timestamped agent activity lines (similar to triage left panel)
  - Footer: "Seeding in progress — agent is exploring your repository"
  - Reuse `TriageActivity` list rendering pattern from `draw_triage_frame`
- `tui.rs` — Add `draw_seeding_live(activity: &[String])` public function
- `runtime/mod.rs` — Add `draw_seeding` method to `Terminal` trait and `ProductionTerminal` impl, delegating to `draw_seeding_live`
- `startup.rs` — Modify `run_seed_with_heartbeat` to call `runtime.terminal.draw_seeding(activity)` instead of `progress(detail)` when TTY. Accumulate activity lines (capped at ~20) and push each agent event summary.
- `lib.rs:474-491` — When seeding is triggered, call a new `run_seeding_with_screen` wrapper that uses the seeding screen instead of `draw_boot_stage`

**Success criteria**:
- During seeding, a dedicated "Seeding your backlog" screen appears
- Agent commands stream in real-time
- Screen transitions to dashboard (auto) or approval (review) after seeding completes

**Confirmation gate**: Build + test green

---

### Phase 4: Approval flow — task-by-task review screen

**Goal**: After seeding completes in review mode, show tasks one at a time for Keep/Discard.

**Changes**:

- `startup.rs` — For the approval path, use the dry-run/JSON seeding path (`recommend_seed_tasks_with_events`) instead of the direct-write path. This returns `Vec<SeedTask>` with `title`, `details`, `rationale`.
- `tui.rs` — Add `draw_seed_review_frame`:
  - Header: "GARDENER  review backlog  (3/10)"
  - Body: bordered card showing:
    - **Title** (bold, white)
    - **Details** (gray)
    - **Why this helps agents** — the `rationale` field (cyan/accent)
    - **Priority** badge (P0/P1/P2 with color)
  - Footer: `[k] Keep    [d] Discard    [q] Discard remaining & finish`
- `tui.rs` — Add `run_seed_review_wizard(tasks: &[SeedTask]) -> Vec<bool>`:
  - Blocking event loop (like `run_repo_health_wizard`)
  - Iterates tasks, renders each with `draw_seed_review_frame`
  - `k` → mark keep, advance; `d` → mark discard, advance; `q` → discard all remaining, exit
  - Returns a `Vec<bool>` parallel to input tasks (true = keep)
- `startup.rs` — After review, insert kept tasks via `store.upsert_task()` with appropriate `NewTask` conversion from `SeedTask`
- `seeding.rs` — Update the seeding prompt's rationale instruction: "rationale must explain why this task makes coding agents more effective in this repository"

**Success criteria**:
- After seeding, review screen shows tasks one at a time with 1/N counter
- Keep/Discard hotkeys work
- Only kept tasks appear in backlog
- If all discarded, gardener exits cleanly

**Confirmation gate**: Build + test green, manually verify review flow

---

### Phase 5: Wire seeding preference into startup flow

**Goal**: Connect the onboarding preference to the seeding execution path.

**Changes**:

- `startup.rs` — Add a new public function `run_interactive_seeding` that:
  1. Reads `backlog_approval` from the profile
  2. If auto-seed: use direct-write path (v2) with seeding screen, then proceed
  3. If review: use dry-run path (v1) with seeding screen, then call `run_seed_review_wizard`, insert kept tasks, proceed or exit
- `lib.rs:474-491` — Replace current seeding call with `run_interactive_seeding` when TTY. Keep existing behavior for non-TTY (auto-seed silently).
- `startup.rs` — The seeding screen replaces `draw_boot_stage("BACKLOG_SYNC", ...)` calls during seeding. After seeding, transition to dashboard boot stages as before.

**Success criteria**:
- Auto-seed preference: seeding screen → dashboard
- Review preference: seeding screen → review wizard → dashboard (or exit if empty)
- Non-TTY: unchanged behavior (silent auto-seed)

**Confirmation gate**: Build + test green, both paths verified

---

### Phase 6: Universal shutdown summary

**Goal**: All exit paths (completion, empty backlog, q/Ctrl+C, error) show a rich summary.

**Changes**:

- `worker_pool.rs` — Add `ShutdownSummary` struct:
  ```rust
  struct ShutdownSummary {
      completed: usize,
      target: usize,
      merged: usize,   // count of tasks that reached merge_pending→complete
      failed: usize,
      total_runtime_secs: u64,
  }
  ```
- `worker_pool.rs` — Track `merged` count: increment when a merge completes successfully (line ~880 where `completed += 1` after merge)
- `worker_pool.rs:1033-1034` — Remove early return on `quit_requested`. Fall through to the shutdown screen with the same summary logic.
- `worker_pool.rs:1046-1064` — Rewrite shutdown message to include:
  - Tasks completed / target
  - Tasks merged (PRs landed)
  - Tasks failed / unresolved (if any)
  - Clean formatting
- `tui.rs:draw_shutdown_frame` — Update to handle multi-line summary with section formatting (already supports multi-line paragraph, just needs better content)
- Wire the summary rendering for all four exit paths:
  1. Normal completion (`completed >= target`)
  2. Empty backlog (`completed < target`)
  3. User quit (q/Ctrl+C) — **new**: now shows shutdown screen before returning
  4. Error — existing error screen, add summary stats to message

**Success criteria**:
- q/Ctrl+C shows shutdown screen with summary before exiting
- Normal completion shows merge count
- All exit paths show consistent summary format

**Confirmation gate**: Build + test green, verify all exit paths

---

## Testing Strategy

### Unit tests
- Wizard step count and new Backlog step rendering
- `RepoHealthWizardAnswers` serialization with `backlog_approval`
- `UserValidated` backward compat (missing field defaults to `false`)
- `ShutdownSummary` formatting
- Removal: delete all `fallback_seed_tasks` and `fallback_from_quality_doc` tests

### Integration tests
- Seeding with empty backlog triggers seeding screen (mock terminal)
- Approval path: `run_seed_review_wizard` with fake key events
- Auto-seed path: direct insert verified in DB
- Quit during worker pool shows shutdown summary

### Manual verification
- Full onboarding flow with new wizard step
- Seeding screen renders with live activity
- Review flow: keep/discard cycle
- q/Ctrl+C during work shows summary
- Existing profiles without `backlog_approval` field load correctly

## References

- `startup.rs:1087-1156` — fallback_seed_tasks (to delete)
- `tui.rs:2015-2213` — existing wizard (pattern for new step)
- `tui.rs:1337-1442` — triage screen (pattern for seeding screen)
- `worker_pool.rs:1033-1064` — current shutdown (to rewrite)
- `seed_runner.rs:14-23` — SeedTask struct
- `repo_intelligence.rs:39-52` — UserValidated struct
