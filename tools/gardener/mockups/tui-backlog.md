# TUI Rewrite Backlog

Sequential tickets. Each is a self-contained block of work.
Reference material: `tools/gardener/mockups/live-queue-mock.html` and `tools/gardener/mockups/live-queue-terminal-requirements.md`.

---

## GARD-01: Replace TUI data model with new view-state structs

**What:** Delete the existing `WorkerRow`, `QueueStats`, `BacklogView`, `ProblemClass`, and `RepoHealthWizardAnswers` structs from `tools/gardener/src/tui.rs`. Replace them with the view-state model defined in requirements section 9. The new structs are the single source of truth for all rendering.

**Structs to create:**

```rust
enum UiMode { Triage, Work }

struct TriageStage { label: String, state: StageState }
enum StageState { Done, Current, Future }

struct TriageActivity { timestamp: String, message: String }
struct TriageArtifact { label: String, value: String }

struct StartupHeadline {
    spinner_frame: usize,
    verb: String,           // chosen once from: Scanning, Seeding, Pruning, Cultivating, Grafting, Harvesting
    startup_active: bool,   // true during first ~4.5s, then false forever
    ellipsis_phase: u8,     // cycles 0/1/2 continuously (maps to "." / ".." / "...")
}

enum WorkerState { Doing, Reviewing, Idle }

struct WorkerMetrics { total: usize, doing: usize, reviewing: usize, idle: usize }

struct Worker {
    name: String,           // equipment name like "Lawn Mower", "Hedge Trimmer"
    state: WorkerState,
    task: String,
    activity: Vec<ActivityEntry>,    // {timestamp, message}
    commands: Vec<CommandEntry>,     // {timestamp, command}
    commands_expanded: bool,
}

struct BacklogItem { title: String, priority: Priority }
enum Priority { P0, P1, P2 }
```

Also create a top-level `AppState` struct that holds: `ui_mode`, four triage stages (hardcoded labels, dynamic states), `triage_activity: Vec<TriageActivity>`, `triage_artifacts: Vec<TriageArtifact>`, `startup_headline: StartupHeadline`, `workers: Vec<Worker>`, `backlog: Vec<BacklogItem>`, `selected_worker: usize` (for scroll/focus), and `terminal_width`/`terminal_height`.

Add a `WorkerMetrics::from_workers(workers: &[Worker]) -> WorkerMetrics` function that derives counts from the worker list (not hardcoded). Unit test it: given 3 Doing, 2 Reviewing, 1 Idle → total=6, doing=3, reviewing=2, idle=1.

**Acceptance:**
- All old structs removed. All new structs compile.
- `WorkerMetrics::from_workers` has a passing unit test.
- `AppState::default()` produces a valid initial state (Triage mode, four stages with first as Current, empty workers/backlog).
- No rendering code yet — this is pure data model.

---

## GARD-02: Implement global frame — header, footer, and mode switching

**What:** Delete the existing `draw_dashboard_frame` and `draw_report_frame` rendering functions. Build the new top-level frame that wraps both modes.

**Header (top row):** `GARDENER` in accent color (cyan/bright blue), right-aligned mode indicators showing `[1] Triage` and `[2] In Progress` where the active mode is highlighted and the inactive one is dimmed. The header sits in a single-line area with a bottom border.

**Footer (bottom row):** `Keys: q quit  1 triage  2 work  v view report  g regenerate` in muted color. Always visible in both modes. Single-line area with a top border.

**Mode switching:** Pressing `1` sets `AppState.ui_mode = Triage`, pressing `2` sets `AppState.ui_mode = Work`. The body area between header and footer renders different content based on mode. For now, just render placeholder text: "Triage view placeholder" or "Work view placeholder".

**Layout:** Use ratatui `Layout` with vertical constraints: `Length(1)` for header, `Min(1)` for body, `Length(1)` for footer.

**Key handling:** Build an event loop that reads crossterm key events. `q` quits. `1` switches to triage. `2` switches to work. Wire this into the existing `with_live_terminal` pattern (or replace it — whatever's cleaner).

**Acceptance:**
- App launches into alternate screen showing header + placeholder body + footer.
- Pressing `1` and `2` swaps the body placeholder text.
- Pressing `q` exits cleanly (leaves alternate screen, restores cursor).
- Header never shows tagline/marketing text.
- Footer is visible in both modes.
- Snapshot test: render at 80x24, assert header contains "GARDENER", footer contains "Keys:", body contains correct placeholder for each mode.

---

## GARD-03: Build Triage mode — Now card and progress rail

**What:** Replace the triage body placeholder from GARD-02 with the actual triage Now card and progress rail.

**Now card:** Rendered as a bordered panel at the top of the triage body. Contains three lines:
1. `Now` — muted/dim uppercase label
2. `Triage and onboarding` — bright, large-feeling text (bold + white)
3. `Reading repo signals, discovering standards, and building a quality-ranked backlog.` — slightly dimmed body text

**Progress rail:** Below the Now card text, a horizontal row of stage pills with arrows between them. Render from `AppState.triage_stages` (4 items). Format: `Scan repository shape → Detect tools and docs → Build project profile → Seed prioritized backlog`

Stage styling based on `StageState`:
- `Done` — dim gray text, dim border
- `Current` — bright cyan/white text, bright border (stands out)
- `Future` — dim gray text, dim border (same as Done visually)

The arrow `→` between stages is always dim gray.

If the terminal is too narrow to fit all stages on one line, let them wrap naturally (ratatui `Line` with spans will handle this).

**Acceptance:**
- Switching to triage mode shows the Now card with exact text above.
- Progress rail shows 4 stages with arrows between them.
- Changing a stage's `StageState` in `AppState` changes its visual styling.
- Snapshot test: render triage mode at 100x24 with stages [Done, Done, Current, Future], assert "Now" appears, assert "Triage and onboarding" appears, assert "Build project profile" is rendered (the current stage), assert `→` separators appear.

---

## GARD-04: Build Triage mode — Live activity and Triage artifacts cards

**What:** Below the Now card + progress rail, render a two-column layout with "Live activity" on the left and "Triage artifacts" on the right.

**Live activity card (left, ~60% width):** Bordered panel titled `LIVE ACTIVITY` (uppercase, accent color). Body is a list of `TriageActivity` entries from `AppState`. Each entry renders as: `HH:MM:SS  <message>` where timestamp is in a blue/accent tint and message is normal text. Entries appear in chronological order (oldest first). If no entries, show "Waiting for activity..."

**Triage artifacts card (right, ~40% width):** Bordered panel titled `TRIAGE ARTIFACTS` (uppercase, accent color). Body is a list of `TriageArtifact` entries from `AppState`. Each renders as: `<label> ............. <value>` where label is left-aligned, value is right-aligned, and dots fill the gap. If no entries, show "No artifacts yet."

**Responsive:** If terminal width < 80 columns, stack the two cards vertically (activity on top, artifacts below) instead of side-by-side. Use ratatui `Layout` with direction conditional on width.

**Acceptance:**
- Two cards render side-by-side at width >= 80.
- Two cards stack vertically at width < 80.
- Activity entries show timestamps in accent color.
- Artifacts show label-value pairs with visual separation.
- Snapshot tests at 100x24 (side-by-side) and 60x24 (stacked).

---

## GARD-05: Build Work mode — Now card with spinner, verb headline, and metrics

**What:** Replace the work body placeholder from GARD-02 with the In Progress Now card.

**Now card structure (bordered panel, top of work body):**
1. `Now` — muted/dim uppercase label (same style as triage)
2. Headline: `<spinner> <verb><ellipsis>` where:
   - `spinner` — a braille character from `[⠋, ⠙, ⠸, ⠴, ⠦, ⠇]`, cyan colored
   - `verb` — one of `[Scanning, Seeding, Pruning, Cultivating, Grafting, Harvesting]`, bright white bold
   - `ellipsis` — cycles through `.`, `..`, `...` continuously
3. `Working the queue in priority order and showing exactly what each worker is doing.` — dimmed body text
4. Metrics row: `<N> parallel workers  <N> doing  <N> reviewing  <N> idle` — each as a pill-like span. The number is bright, the label is dimmed. Values come from `WorkerMetrics::from_workers()`.

**Startup animation logic (driven by `AppState.startup_headline`):**
- On app start, pick a random verb and store it in `startup_headline.verb`. Never change it after.
- `startup_headline.startup_active` starts `true`. A tick handler advances `spinner_frame` every 150ms. After 30 ticks (~4.5s), set `startup_active = false` and stop advancing the spinner frame (freeze it).
- `startup_headline.ellipsis_phase` increments independently on a ~400ms cadence and never stops. Render as: phase 0 → `.`, phase 1 → `..`, phase 2 → `...`, then wraps.

**Tick integration:** The main event loop needs a tick timer (e.g., crossterm `poll` with timeout) that fires every ~150ms to drive both the spinner and ellipsis animations. The tick handler updates `AppState.startup_headline` fields and triggers a re-render.

**Acceptance:**
- Switching to work mode shows the Now card with spinner + verb + dots.
- Spinner animates for ~4.5s then freezes.
- Verb never changes after startup.
- Ellipsis cycles continuously even after spinner stops.
- Metrics row shows counts derived from the worker list (test with mock workers).
- Unit test: `StartupHeadline` after 30 ticks has `startup_active == false`.
- Snapshot test: work mode Now card at tick 0 (spinner active) and tick 35 (spinner frozen).

---

## GARD-06: Build Work mode — Workers panel with scrollable viewport

**What:** Below the Now card in work mode, render the Workers section as a scrollable list of worker entries.

**Section header:** `WORKERS` (uppercase, accent color, like the card titles in triage).

**Worker entry layout (each worker gets a bordered sub-panel):**
- **Header line:** `<name>` left-aligned in cyan/bold, `<state>` right-aligned. State colors: Doing → yellow, Reviewing → blue, Idle → dim gray.
- **Task line:** Current task summary text, slightly dimmed. E.g., `P0: startup validation recovery`
- **Activity log:** 2-4 most recent `ActivityEntry` items, each as `HH:MM:SS  <message>`. Timestamps in blue tint. Entries separated by thin dim lines or just newlines.

**Worker names:** Use equipment/garden names from the requirements: Lawn Mower, Leaf Blower, Hedge Trimmer, Edger, String Trimmer, Wheelbarrow, Seed Spreader, Pruning Shears, Sprinkler. Never use `SYS`, `Worker X`, or `w1`-style IDs in the UI.

**Scrolling:** The workers section gets a fixed viewport height: `min(terminal_height - 12, total_worker_content_height)`. Roughly: leave room for header (1), Now card (~6), backlog (~6), footer (1). The remaining rows go to workers. Track `selected_worker` index in `AppState`. Up/down arrow keys (or `j`/`k`) move selection. The viewport scrolls to keep the selected worker visible. Highlight the selected worker's border more brightly.

**Stability:** When worker data updates (new activity entries, state changes), preserve the current `selected_worker` index and scroll offset. Don't jump to top on every update.

**Acceptance:**
- Workers render as distinct bordered entries within the workers section.
- Each entry shows name, state badge, task, and activity log with timestamps.
- Arrow keys / j/k scroll through workers, viewport follows selection.
- No `SYS` or numeric worker IDs appear anywhere.
- With 9 workers at 80x24, not all workers are visible simultaneously (scrolling works).
- Snapshot test: 3 workers rendered at 100x30, selected_worker=1 (second worker highlighted).

---

## GARD-07: Add per-worker command detail toggle

**What:** Each worker entry can optionally show a "command detail" section below its activity log.

**Collapsed state (default):** Below the activity log, show a dim line: `▸ Optional command detail` (or `► Optional command detail`). This hints that detail is available but doesn't take space.

**Expanded state:** When toggled open, the line becomes `▾ Optional command detail` and below it renders `CommandEntry` items: `HH:MM:SS  <command>`. Commands are monospace, slightly dimmer than activity entries. Long commands truncate with `…` at the terminal edge (timestamps always remain visible — truncate from the right end of the command text, not the timestamp).

**Toggle mechanism:** When a worker is selected (focused via scroll from GARD-06), pressing `Enter` or `e` toggles `commands_expanded` for that worker. Each worker's expansion state is independent.

**Curated activity stays visible:** The activity log entries above the command detail section are always shown regardless of expansion state. Command detail is additive, not a replacement.

**Acceptance:**
- Collapsed workers show the `▸ Optional command detail` hint line.
- Pressing `Enter`/`e` on selected worker toggles expansion.
- Expanded view shows timestamped commands, truncated with `…` if too wide.
- Expanding one worker doesn't affect others.
- Activity log is visible in both states.
- Snapshot test: one worker expanded, one collapsed, verify command lines appear only in expanded worker.

---

## GARD-08: Build Work mode — Backlog panel with priority badges

**What:** Below the workers section, render the Backlog as the bottom panel in work mode.

**Section header:** `BACKLOG (PRIORITY ORDER)` (uppercase, accent color).

**Entry format:** Each `BacklogItem` renders as a single line: `<priority badge>  <title>`. Priority badge is a short colored tag:
- `P0` — red/bright red (high severity, matches `--bad` from the HTML mock: `#ff7a7a`)
- `P1` — yellow/amber (medium, matches `--warn`: `#ffcf69`)
- `P2` — green (lower, matches `--good`: `#7fe694`)

Entries are in strict priority order (P0 first, then P1, then P2). Within same priority, preserve the order from `AppState.backlog`.

**Layout:** The backlog panel sits below workers in the vertical stack. It should show as many items as fit in the remaining space. If the backlog is longer than available rows, show what fits and add a `... and N more` line at the bottom.

**Vertical stack enforcement:** The work mode body layout is strictly: Now card → Workers → Backlog, top to bottom. Never side-by-side. Use ratatui `Layout::vertical` with constraints: `Length(~7)` for Now card, `Min(8)` for workers, `Length(backlog_rows + 2)` for backlog (capped).

**Acceptance:**
- Backlog renders below workers, never beside them.
- Priority badges are colored correctly (P0 red, P1 yellow, P2 green).
- Entries appear in priority order.
- With 6 backlog items at 80x24, all or most are visible.
- Snapshot test: backlog with [P0, P1, P1, P2, P2] items, verify ordering and color assertions.

---

## GARD-09: Wire live data feeds into the new AppState

**What:** Connect the actual runtime data sources to the new `AppState` so the TUI shows real information instead of hardcoded mock data.

This ticket depends on understanding how the existing code feeds data into the old TUI. Look at how `draw_dashboard_live` is called — its callers pass `WorkerRow`, `QueueStats`, `BacklogView`. Find those call sites and adapt them to populate the new `AppState` structs instead.

**Mapping:**
- Old `WorkerRow.worker_id` → assign an equipment name from a fixed list (map worker index to name, or hash the ID to pick one deterministically)
- Old `WorkerRow.state` → map to `WorkerState` enum (doing/gitting/planning → Doing, reviewing → Reviewing, idle → Idle, etc.)
- Old `WorkerRow.task_title` → `Worker.task`
- Old `WorkerRow.tool_line` / `WorkerRow.breadcrumb` → convert to `ActivityEntry` items with timestamps
- Old `QueueStats` → no longer used directly (metrics derived from workers)
- Old `BacklogView.in_progress` / `BacklogView.queued` → parse into `BacklogItem` with priority extracted from the `P0`/`P1`/`P2` prefix

**Triage data:** Wire the seeding/triage activity stream (wherever that currently emits events) into `AppState.triage_activity` and `AppState.triage_artifacts`. Update `triage_stages` state as triage progresses.

**Acceptance:**
- Running gardener with real worker processes shows live worker data in the new UI.
- Worker names are equipment-themed, not raw IDs.
- Backlog items show parsed priorities.
- Triage mode shows real seeding activity if in triage phase.

---

## GARD-10: Handle terminal resize and responsive layout

**What:** Make the TUI respond to terminal resize events without crashing or leaving artifacts.

**Resize handling:** The crossterm event stream includes `Event::Resize(width, height)`. On resize: update `AppState.terminal_width` and `AppState.terminal_height`, clear the terminal buffer, and force a full re-render. The existing `with_live_terminal` already detects resize — adapt or replace that logic for the new event loop.

**Responsive breakpoints:**
- Width < 80: Triage cards stack vertically (already handled in GARD-04). Work mode workers section gets fewer visible rows.
- Width >= 80: Triage cards side-by-side. Workers get more room.
- Width >= 120: Workers panel gets extra height allocation.

**No jitter:** Resize should produce a clean frame, not a flash of partially-rendered content. Clear first, then draw.

**Acceptance:**
- Resizing the terminal redraws cleanly with no artifacts.
- Triage two-column layout collapses to single-column below 80 columns.
- App doesn't panic on very small terminals (e.g., 40x10) — degrade gracefully (show what fits, skip optional sections).

---

## GARD-11: Delete dead code from old TUI

**What:** Clean up everything from the old TUI that's no longer used after GARD-01 through GARD-10.

**Delete:**
- `classify_problem`, `ProblemClass`, `requires_human_attention`, `describe_problem_for_human` — the "Problems Requiring Human" panel is gone
- `humanize_state`, `humanize_action`, `humanize_breadcrumb`, `title_case_words` — replaced by the new data model's enum rendering
- `render_dashboard` (the `TestBackend` snapshot function) — replaced by new snapshot approach
- `panel_block` if unused
- `run_repo_health_wizard` and `RepoHealthWizardAnswers` — move to a separate module if still needed, but get it out of `tui.rs`
- Old snapshot tests in `mod tests` that test deleted functions
- Any imports that become unused after deletions

**Keep:** `with_live_terminal`, `close_live_terminal`, `teardown_terminal` (or their replacements), and `render_report_view`/`draw_report_frame` if the report view is still needed.

**Acceptance:**
- `cargo build` succeeds with no dead-code warnings from `tui.rs`.
- `cargo test` passes (old tests deleted, new tests from previous tickets still pass).
- No `SYS`, `WorkerRow`, `QueueStats`, or `ProblemClass` references remain in `tui.rs`.

---

## GARD-12: Snapshot and interaction tests for the full TUI

**What:** Lock the new UI behavior with comprehensive tests. Use ratatui's `TestBackend` to render frames and assert on content.

**Snapshot tests (render `AppState` → assert string content):**
1. **Triage baseline:** Default `AppState` in Triage mode at 100x24. Assert: "GARDENER" in header, "Now" label, "Triage and onboarding", all 4 stage labels present, "→" separators, "Live activity" and "Triage artifacts" card titles, footer with "Keys:".
2. **Work mode — spinner active:** `AppState` in Work mode, `startup_active=true`, tick 5. Assert: spinner glyph present, verb present, "Now" label, metrics row with numbers.
3. **Work mode — spinner frozen:** Same but tick 35. Assert: spinner glyph still present (frozen), verb unchanged.
4. **Work mode — workers with scroll:** 6 workers, `selected_worker=3`, at 80x24. Assert: selected worker's name appears, worker states visible, not all 6 workers fully visible (proving scroll is needed).
5. **Work mode — command detail expanded:** 2 workers, worker 0 has `commands_expanded=true` with 3 commands. Assert: command timestamps and text appear for worker 0, `▸ Optional command detail` appears for worker 1.
6. **Backlog rendering:** 4 backlog items [P0, P1, P1, P2]. Assert: "P0" appears before "P1" which appears before "P2", all titles present.

**Interaction tests (simulate key events → check `AppState` changes):**
1. Press `1` → `ui_mode == Triage`. Press `2` → `ui_mode == Work`.
2. In work mode with 5 workers: press `j` 3 times → `selected_worker == 3`. Press `k` once → `selected_worker == 2`.
3. Press `Enter` on selected worker → `commands_expanded` toggles.
4. Press `q` → app exits (test that the quit flag is set).

**Regression guards (explicit assertions):**
1. Render work mode → assert output does NOT contain "SYS".
2. Render header → assert output does NOT contain any string from a banned-copy list (e.g., no tagline text).
3. Render work mode → verify vertical order: "Now" appears at a lower row number than "WORKERS", which appears at a lower row number than "BACKLOG".

**Acceptance:**
- All tests pass.
- Tests are in `tui.rs` `mod tests` or a separate `tui_tests.rs` file.
- `cargo test` runs them all.
