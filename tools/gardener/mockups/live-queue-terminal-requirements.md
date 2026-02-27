# Gardener Terminal UI Requirements (Extracted From `live-queue-mock.html`)

## 1. Purpose
This document is the implementation contract for the terminal version of the UI represented by:

- `tools/gardener/mockups/live-queue-mock.html`

The goal is to preserve the **experience model** from the HTML mock, not merely its raw layout.

The UI must feel like:

1. A coherent narrative of what Gardener is doing on the user's behalf.
2. A clear distinction between onboarding/triage and active execution.
3. A trustworthy live view of worker behavior with optional low-level command detail.
4. A practical terminal workflow with persistent hotkeys.

## 2. Primary Product Principles
These are non-negotiable.

1. The UI is a user experience, not an engine dashboard.
2. The top-level question answered at all times is: "What is Gardener doing right now?"
3. During triage, progress should be explicit and confidence-building.
4. During execution, priority ordering and worker activity should be explicit and inspectable.
5. Raw command output is optional detail, not the default primary narrative.
6. Corporate/motto copy is excluded.

## 3. Scope and View Model
The UI has two primary modes:

1. `Triage/Onboarding`
2. `In Progress`

Only one mode is visible at a time.

## 4. Global Frame Requirements
The top frame has:

1. Brand title: `Gardener`
2. Mode switch controls:
   - `Triage/Onboarding`
   - `In Progress`

The top frame must **not** include secondary marketing tagline text.

The bottom frame has persistent hotkeys text:

- `Keys: q quit  v view report  g regenerate`

This footer is always visible in both modes.

## 5. Triage/Onboarding Requirements

### 5.1 "Now" card content
Must include:

1. Section label: `Now`
2. Title: `Triage and onboarding`
3. Description text:
   - `Reading repo signals, discovering standards, and building a quality-ranked backlog.`

### 5.2 Triage progress rail
The triage progress rail is linear and shows four stages in order with arrows between stages:

1. `Scan repository shape`
2. `Detect tools and docs`
3. `Build project profile`
4. `Seed prioritized backlog`

State semantics:

1. Non-active stages are gray.
2. Active stage is highlighted.
3. Current mock state marks:
   - first two as `done` (gray, not green)
   - third as `current` (highlight)
   - fourth as `future` (gray)

Arrow separators (`→`) visually indicate sequence.

### 5.3 Triage body layout
Two-card layout:

1. Left card: `Live activity`
2. Right card: `Triage artifacts`

On narrow width, stack vertically.

### 5.4 Live activity list contract
`Live activity` entries must be:

1. Timestamped.
2. Human-readable, action-level summaries.
3. Ordered chronologically.

Current seed entries:

1. `14:18:11 Loaded AGENTS.md instructions for runtime + skills.`
2. `14:18:17 Detected Rust runtime entrypoint and startup policy.`
3. `14:18:26 Checking quality-grade artifact and validation command wiring.`
4. `14:18:32 Classifying domains by risk to seed first backlog candidates.`

### 5.5 Triage artifacts list contract
This card must not duplicate the progress rail semantics.
It is output-oriented, with labels + values.

Current seed entries:

1. `Detected domains` -> `12`
2. `Standards/docs indexed` -> `8`
3. `Candidate tasks drafted` -> `17`
4. `Prioritized ready for execution` -> `9`

## 6. In Progress Requirements

### 6.1 "Now" card content
Must include:

1. Section label: `Now`
2. Headline that is a spinner + verb + animated trailing dots.
3. Description text:
   - `Working the queue in priority order and showing exactly what each worker is doing.`
4. Worker parallelism summary pills (see section 6.3).

The headline must **not** include the literal static word `Gardening`.
The spinner+verb is the headline experience.

### 6.2 Spinner and verb behavior
Spinner headline contract:

1. Spinner glyph field exists.
2. Verb field exists.
3. Ellipsis field exists and visually animates.

Verb list for startup selection:

1. `Scanning`
2. `Seeding`
3. `Pruning`
4. `Cultivating`
5. `Grafting`
6. `Harvesting`

Behavior rules:

1. Verb is selected once at startup.
2. Verb does not rotate after startup.
3. Spinner animates only during startup window.
4. Startup spinner frame list:
   - `⠋`, `⠙`, `⠸`, `⠴`, `⠦`, `⠇`
5. Startup animation cadence from mock:
   - every `150ms`
   - stop after `30` ticks (~4.5s)
6. Ellipsis (`...`) animates continuously as independent visual punctuation.

Terminal guidance:

1. If full dot-opacity animation is not feasible, approximate with cycling:
   - `.  `
   - `.. `
   - `...`
2. Keep verb stable post-startup.

### 6.3 Parallel worker metrics
Metrics row must include:

1. `<N> parallel workers`
2. `<N> doing`
3. `<N> reviewing`
4. `<N> idle`

Computation contract:

1. These counts are derived from live worker state, not hardcoded.
2. `parallel workers` equals total worker cards/rows currently represented.
3. State buckets are case-insensitive by canonical state names:
   - `doing`
   - `reviewing`
   - `idle`

### 6.4 In-progress layout hierarchy
Use vertical stack order:

1. `Now` card (spinner + metrics)
2. `Workers` panel
3. `Backlog (priority order)` panel

This stack is required. Do not place backlog beside workers in default execution mode.

### 6.5 Workers panel behavior
Workers panel requirements:

1. Full-width section in middle of stack.
2. Capped visible height with vertical scrolling when content exceeds cap.
3. Can represent many workers (example density target: around 10 workers).

Terminal implementation guidance:

1. Define a fixed viewport height for worker list region based on terminal rows.
2. Implement keyboard scrolling in that region.
3. Maintain stable selection/scroll offset when worker updates stream in.

### 6.6 Worker row/card content contract
Each worker entry includes:

1. Worker name using lawn/garden equipment naming scheme.
2. Worker state (`Doing`, `Reviewing`, `Idle`).
3. Current task summary.
4. Curated activity log entries with timestamps.
5. Optional command detail section (expandable) with timestamped raw commands.

No `SYS` naming is used in worker identity.

Current name style examples:

1. `Lawn Mower`
2. `Leaf Blower`
3. `Hedge Trimmer`
4. `Edger`
5. `String Trimmer`
6. `Wheelbarrow`
7. `Seed Spreader`
8. `Pruning Shears`
9. `Sprinkler`

### 6.7 Optional command detail contract
This is critical and must be preserved.

Rules:

1. Command transcript is optional detail, not always expanded.
2. Command detail header text:
   - `Optional command detail`
3. When expanded, command lines are shown with timestamps.
4. Command lines may be truncated/ellipsized at viewport edge.
5. Curated activity remains visible even when command detail is collapsed.

Terminal guidance:

1. Provide an explicit toggle hotkey for expanding/collapsing selected worker details.
2. Keep command detail per-worker state independent.

### 6.8 Backlog panel requirements
Backlog section title:

- `Backlog (priority order)`

Requirements:

1. List is in strict priority order (highest first).
2. Priority badges shown as `P0`, `P1`, `P2`.
3. Backlog appears below workers in execution mode.

Current seed entries:

1. `Add startup validation failure recovery task` -> `P0`
2. `Harden worktree cleanup on interrupted sessions` -> `P1`
3. `Improve prompt packet size guardrails` -> `P1`
4. `Add scheduler claim/dispatch timeline view` -> `P1`
5. `Refine TUI copy for worker events` -> `P2`
6. `Improve report view typography` -> `P2`

## 7. Copy and Language Rules
Use direct, operational language.

Required:

1. Human-readable state/action descriptions.
2. Timestamp-first activity lines.
3. Clear "what is happening now" text.

Forbidden:

1. `SYS` worker naming.
2. Corporate slogan copy.
3. Jargon-heavy labels with no user-facing meaning.

## 8. Interaction Requirements

### 8.1 Mode switching
HTML uses two buttons and hash state.

Terminal equivalent must provide deterministic mode switching:

1. Key action to go to `Triage/Onboarding`.
2. Key action to go to `In Progress`.

### 8.2 Worker scroll and detail control
Terminal must support:

1. Scroll workers list.
2. Focus worker entry.
3. Toggle optional command detail for focused worker.

### 8.3 Startup animation lifecycle
On app start:

1. Choose verb once.
2. Run startup spinner for bounded duration.
3. End startup spinner.
4. Keep stable post-startup headline verb.

## 9. Data Model Requirements
Define explicit view model structures to avoid stringly UI state.

Minimum model components:

1. `ui_mode`: `triage | work`
2. `triage_progress`: ordered list of stage items with `state` (`done|current|future`)
3. `triage_activity`: list of `{timestamp, message}`
4. `triage_artifacts`: list of `{label, value}`
5. `startup_headline`: `{spinner_frame, verb, startup_active}`
6. `worker_metrics`: `{total, doing, reviewing, idle}`
7. `workers`: ordered list of:
   - `name`
   - `state`
   - `task`
   - `activity[]` entries `{timestamp, message}`
   - `commands[]` entries `{timestamp, command}`
   - `commands_expanded` boolean
8. `backlog`: ordered list of `{title, priority}`
9. `footer_hotkeys`: display string list

## 10. Styling Semantics To Preserve In Terminal
Do not attempt pixel parity with HTML/CSS.
Preserve meaning and hierarchy.

Preserve:

1. Distinct panel boundaries.
2. Strong headline hierarchy (`Now` > title > support text).
3. Gray non-active triage steps and highlighted active step.
4. Equipment names visually distinct from task text.
5. Timestamp tint/format consistency.
6. Priority color cues:
   - P0 high severity
   - P1 medium
   - P2 lower

## 11. Terminal-Specific Constraints and Guidance

### 11.1 Responsiveness
The HTML has a breakpoint at 1120px.
Terminal equivalent must have small/medium/large layout behavior.

Recommendation:

1. Small terminal: stack everything vertically.
2. Medium terminal: triage two-column optional; work remains stacked.
3. Large terminal: triage two-column; work stacked with larger worker viewport.

### 11.2 Overflow strategy

1. Worker activity lines and commands may exceed width.
2. Truncate with ellipsis or soft-wrap based on chosen policy.
3. Keep timestamps visible when truncating.

### 11.3 Update cadence
Avoid visual jitter.

1. Coalesce frequent updates.
2. Keep stable ordering unless explicitly changed by scheduler.
3. Preserve scroll position across refreshes.

## 12. Acceptance Criteria
Implementation is correct only if all checks below pass.

### 12.1 Mode and frame

1. Header shows `Gardener` and two mode controls only.
2. Footer shows persistent hotkeys in both modes.
3. Switching modes swaps visible content without stale bleed-through.

### 12.2 Triage mode

1. `Now` card title and copy match expected semantics.
2. Progress rail shows arrows and exactly one active stage.
3. Non-active stages are gray.
4. Live activity entries are timestamped and chronological.
5. Triage artifacts show output metrics, not duplicate progress copy.

### 12.3 Work mode headline

1. Headline uses spinner + verb + animated dots.
2. Verb is selected once per startup.
3. Verb does not rotate continuously.
4. Spinner animates only during startup.

### 12.4 Workers section

1. Workers are labeled with equipment names, not `SYS` or `Worker X`.
2. Middle section is workers and is scrollable when long.
3. Each worker shows timestamped curated activity.
4. Optional command detail can be toggled and shows timestamped commands.

### 12.5 Backlog section

1. Backlog is bottom section in work mode.
2. Entries are shown in priority order with visible priority tag.

### 12.6 Metrics

1. Metrics compute from live worker states.
2. Metrics values update when worker state changes.

## 13. Suggested Test Plan (Terminal)

### 13.1 Unit tests

1. Worker metric derivation from worker list and states.
2. Startup verb selection immutability after initialization.
3. Startup spinner lifecycle timing/state transitions.
4. Triage stage rendering styles for `done/current/future`.
5. Optional command detail toggle state per worker.

### 13.2 Snapshot/render tests

1. Triage view baseline.
2. Work view baseline with startup animation active.
3. Work view baseline after startup animation complete.
4. Work view with multiple workers and scroll offset applied.
5. Work view with command detail expanded for one worker.

### 13.3 Interaction tests

1. Mode switch keybinds.
2. Worker list scrolling.
3. Command detail toggle.
4. Footer hotkeys still visible while scrolling.

### 13.4 Regression tests

1. Ensure no `SYS` appears in user-facing worker labels.
2. Ensure no removed corporate copy reappears in header.
3. Ensure work mode remains vertically stacked (`Now`, `Workers`, `Backlog`).

## 14. Known Implementation Notes From Source Mock

1. Ellipsis animation is CSS-based and infinite. Terminal needs equivalent lightweight pulse/cycle behavior.
2. Startup spinner in source mock stops after ~4.5s and leaves selected verb fixed.
3. Worker metrics are derived from DOM query of `.worker-state` values in the mock; terminal should derive from canonical runtime worker data.
4. Source mock contains one duplicated `workers-list` wrapper in markup. Treat this as mock artifact, not intended product requirement.

## 15. Delivery Checklist For Terminal Port

1. Build view-state structs first.
2. Implement mode switch + static layouts.
3. Implement triage rail semantics and activity feed.
4. Implement startup spinner/verb lifecycle.
5. Implement workers viewport with scroll.
6. Implement optional command detail toggle.
7. Implement derived worker metrics.
8. Implement backlog bottom panel with priority styling.
9. Lock behavior with snapshot and interaction tests.

---

If any ambiguity appears during terminal implementation, prefer this rule:

- Preserve user comprehension over raw telemetry completeness.

