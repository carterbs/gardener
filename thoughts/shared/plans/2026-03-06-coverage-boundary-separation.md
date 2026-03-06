# Coverage Boundary Separation Plan

## Overview

Keep a strict unit-test coverage gate, but only for files where line coverage is a strong proxy for runtime confidence. Move mixed-purpose files toward a clearer split between pure logic/render/state modules and terminal/process orchestration modules, then apply different validation rules to each class.

## Current State Analysis

The TUI is already split into modules, but several files still mix pure logic with live terminal orchestration:

- `tools/gardener/src/tui/terminal.rs:26-105` owns terminal singleton state, raw-mode setup, alternate-screen lifecycle, resize handling, and the `draw_live_frame` wrapper.
- `tools/gardener/src/tui/terminal.rs:107-175` also exposes thin live wrappers plus string-render helpers, while `tools/gardener/src/tui/terminal.rs:177-321` contains pure frame rendering for seeding/shutdown.
- `tools/gardener/src/tui/quality.rs:72-88` owns live drawing orchestration, but `tools/gardener/src/tui/quality.rs:105-249` is pure render/data shaping.
- `tools/gardener/src/tui/wizard.rs:43-129` contains pure answer normalization and key-handling state transitions, while `tools/gardener/src/tui/wizard.rs:269-306` launches the real blocking TTY wizard.
- `tools/gardener/src/tui/seed_review.rs:241-363` contains pure prompt/state transition helpers, while `tools/gardener/src/tui/seed_review.rs:365-412` launches the real blocking TTY wizard.
- `tools/gardener/src/worktree_audit.rs:12-94` is a single orchestration function mixing cwd lookup, production process runner construction, worktree listing, pruning, and logging side effects.

Current tests already reveal the boundary problem:

- `tools/gardener/src/tui/wizard.rs:314-519` and `tools/gardener/src/tui/seed_review.rs:414-625` are strong unit tests against state transitions and rendered output.
- `tools/gardener/tests/tui_live_smoke.rs:3-35` separately exercises live TUI paths under a pseudo-terminal, which is the right shape for boundary validation.
- `tools/gardener/src/tui/terminal.rs:587-596` and `tools/gardener/src/tui/quality.rs:317-323` now use test-only bypass hooks to hit live wrapper lines without a real TTY. Those tests satisfy coverage mechanics, but they are weaker evidence than the pseudo-terminal smoke tests.

The current gates are also flat where the code is not:

- `scripts/test-gardener-coverage.sh:5-105` applies one repository-wide line coverage threshold.
- `tools/gardener/tests/instrumentation_lint.rs:5-43` and `tools/gardener/tests/instrumentation_lint.rs:81-153` apply a per-file instrumentation threshold to a broad set of runtime files, with exclusions managed by filename list rather than by code role.
- `tools/gardener/src/runtime/mod.rs:701-752` calls the live TUI wrappers as orchestration endpoints.
- `tools/gardener/src/startup.rs:357-366` and `tools/gardener/src/startup.rs:1195-1203` call worktree reconciliation and seed-review blocking UI flows.
- `tools/gardener/src/triage_interview.rs:93-104` launches the repo-health wizard directly when attached to a TTY.

## Desired End State

The codebase should separate into two testability classes:

1. `unit-core` files
   Pure parsing, normalization, state transitions, ordering, formatting, and render-model building. These keep a strict changed-file unit coverage gate.

2. `boundary-orchestration` files
   Terminal lifecycle, process spawning, cwd/environment lookup, real git/worktree calls, blocking event loops, and other side-effect-heavy adapters. These are validated primarily by integration-style tests, targeted smoke tests, and instrumentation expectations rather than strict line coverage.

In the end, a reviewer should be able to answer “what gate applies here?” from the file path alone, without reading the implementation.

## Key Discoveries

- `tools/gardener/src/tui/backlog.rs:41-214` is already a clean `unit-core` module: width bucketing, backlog parsing, priority ordering, and bounded list shaping.
- `tools/gardener/src/tui/wizard.rs:68-129` and `tools/gardener/src/tui/seed_review.rs:281-363` are also `unit-core` logic, even though each file later contains a blocking TTY launcher.
- `tools/gardener/src/tui/terminal.rs:48-86`, `tools/gardener/src/tui/wizard.rs:269-306`, `tools/gardener/src/tui/seed_review.rs:365-412`, and `tools/gardener/src/worktree_audit.rs:12-94` are boundary adapters whose value is in correct side effects and runtime wiring, not in raw line execution count.
- `tools/gardener/tests/tui_live_smoke.rs:3-35` already establishes the right validation pattern for boundary TUI code: real PTY execution with narrow scope.
- The current friction is structural, not just numerical. The coverage gate is forcing wrapper-line execution because files still contain both pure logic and side-effect adapters.

## What We’re Not Doing

- No change to runtime behavior in this planning phase.
- No immediate rewrite of `runtime/mod.rs` or `startup.rs` beyond adjusting imports/call sites during later migration.
- No removal of coverage gates. The goal is to retarget them, not weaken them.
- No introduction of a large new test framework if existing `cargo test`, `script`, tempdirs, and current integration suites are sufficient.

## Implementation Approach

Use file-system boundaries, not comments or conventions, as the primary mechanism. Split each mixed file so that the pure logic lives in path prefixes that are always eligible for strict unit coverage, and the live adapters live in path prefixes that are always validated by integration/smoke/orchestration gates.

## Proposed Target Structure

### TUI

- `tools/gardener/src/tui/views/`
  - Pure ratatui frame builders and render helpers.
  - Candidates: seeding/shutdown frames from `terminal.rs`, quality intro/grading frame builders from `quality.rs`, wizard frame builder from `wizard.rs`, seed-review frame builder from `seed_review.rs`.

- `tools/gardener/src/tui/state/`
  - Pure state machines and input reducers.
  - Candidates: `WizardState`, `WizardAction`, `finalize_answers` from `wizard.rs`; `SeedReviewState`, `InputMode`, `apply_input_mode_key`, `apply_review_mode_key`, `handle_seed_review_key`, `finalize_review_decisions` from `seed_review.rs`.

- `tools/gardener/src/tui/live/`
  - Real terminal lifecycle and blocking loops only.
  - Candidates: `with_live_terminal` and wrapper draws from `terminal.rs`; `draw_quality_live` from `quality.rs`; `run_repo_health_wizard`; `run_seed_review_wizard`.

### Runtime / Worktree

- `tools/gardener/src/worktree_audit/model.rs`
  - `WorktreeAuditSummary` and pure summary/result types.

- `tools/gardener/src/worktree_audit/logic.rs`
  - Pure classification helpers, e.g. count stale entries, map prune results to summary, build event payload structs if introduced.

- `tools/gardener/src/worktree_audit/live.rs`
  - cwd lookup, `ProductionProcessRunner`, `WorktreeClient`, `list`, `prune_orphans`, logging dispatch.

This does not require deep nesting everywhere on day one. A lighter version using `*_state.rs`, `*_view.rs`, and `*_live.rs` files under the existing `tui/` directory is acceptable if it preserves obvious gate boundaries.

## Proposed Future Gate Shape

### Gate 1: Repository Line Coverage

Keep the existing repository-wide `cargo llvm-cov` line threshold in `scripts/test-gardener-coverage.sh`, but treat it as a coarse floor rather than the main signal for mixed runtime files.

### Gate 2: Strict Unit Coverage on `unit-core`

Add a changed-file or manifest-driven gate for files in approved `unit-core` paths. Initial target:

- `tools/gardener/src/tui/backlog.rs`
- `tools/gardener/src/tui/state/**`
- `tools/gardener/src/tui/views/**` where the file is pure render logic
- future pure runtime logic files extracted from orchestration modules

Rule:

- changed `unit-core` file must meet `>= 90%` line coverage
- no test-only bypass branches to hit adapter wrappers
- failures report uncovered lines by file

Implementation options:

- preferred: diff-aware changed-file coverage using `cargo llvm-cov --json` plus a small Rust or shell helper
- acceptable first step: manifest of `unit-core` files and a per-file parser over `llvm-cov export`

### Gate 3: Boundary Integration Gate on `boundary-orchestration`

For files under `tui/live/**`, `worktree_audit/live.rs`, and similar adapters:

- require dedicated integration or smoke tests that execute the real boundary
- continue instrumentation expectations where side-effect observability matters
- optionally require each boundary file to declare at least one owning integration test

Examples:

- `tools/gardener/tests/tui_live_smoke.rs` owns live TUI wrappers and blocking wizards
- new temp-repo integration tests should own worktree reconciliation behavior

### Gate 4: Role-Aware Instrumentation Gate

Refactor `tools/gardener/tests/instrumentation_lint.rs` so inclusion is based on file role manifests instead of ad hoc exclusions.

Proposed buckets:

- `instrumented-boundaries`: must keep current instrumentation threshold
- `pure-unit-core`: exempt from instrumentation threshold
- `legacy-mixed`: temporary bucket with explicit migration deadline

## Phased Migration

### Phase 1: Declare Coverage Roles

Create a small manifest checked into the repo, for example:

- `tools/gardener/testability-boundaries.toml`

It should map files or globs into:

- `unit-core`
- `boundary-orchestration`
- `legacy-mixed`

Changes required:

- classify current TUI/runtime/worktree files
- teach the coverage and instrumentation tooling to read the manifest
- fail if a file is unclassified

Success criteria:

- every `tools/gardener/src/**/*.rs` file is assigned a role
- `instrumentation_lint` no longer relies on a growing static exclusion list for these areas
- CI prints the role for each failing file

Confirmation gate:

- manifest validation test passes
- existing `run-validate` still passes with no production behavior change

### Phase 2: Extract TUI Pure State from Live Loops

Split `wizard.rs` and `seed_review.rs` first because they already contain obvious seams.

Changes required:

- move pure state/reducer logic out of `tools/gardener/src/tui/wizard.rs:43-129` and `tools/gardener/src/tui/seed_review.rs:268-363`
- keep blocking TTY loops in dedicated `*_live.rs` files
- keep frame drawing in `*_view.rs` or `views/` files

Success criteria:

- `run_repo_health_wizard` file contains no state-transition logic beyond loop orchestration
- `run_seed_review_wizard` file contains no decision logic beyond loop orchestration
- unit tests move with the state modules and remain green
- PTY smoke tests remain the only tests that need a real terminal path

Confirmation gate:

- existing wizard and seed-review unit tests still pass
- `tools/gardener/tests/tui_live_smoke.rs` still passes

### Phase 3: Extract Shared Live Terminal Adapters

Split `terminal.rs` and `quality.rs` into pure view code vs live terminal adapters.

Changes required:

- move `draw_seeding_frame` / `draw_shutdown_frame` out of `tools/gardener/src/tui/terminal.rs:177-321`
- move `draw_quality_grading_frame`, `draw_quality_intro_frame`, and helpers out of `tools/gardener/src/tui/quality.rs:105-249`
- keep `with_live_terminal`, resize handling, and live wrapper entrypoints in a boundary module

Success criteria:

- live adapter files are mostly thin wrappers and lifecycle management
- pure frame-builder files can be line-covered without bypass hooks
- test-only bypass helpers can be deleted from live adapter files once integration coverage is sufficient

Confirmation gate:

- string-render unit tests cover pure view files
- PTY smoke tests cover live wrappers
- no reduction in observable runtime behavior

### Phase 4: Split Worktree Audit into Logic and Live Adapter

Address `tools/gardener/src/worktree_audit.rs:12-94`.

Changes required:

- extract stale-entry classification and summary shaping into pure helpers
- keep cwd/process/client creation and prune calls in a live adapter file
- add temp-repo or fake-runner integration coverage for prune/list behavior

Success criteria:

- pure worktree summary logic is unit-tested directly
- live worktree reconciliation is exercised through integration tests using temp directories or a fake runner seam
- no need to force line coverage on the adapter wrapper itself

Confirmation gate:

- unit tests cover classification helpers
- integration tests cover list/prune success and failure paths

### Phase 5: Turn on Role-Aware Gates

Once enough files have been moved out of `legacy-mixed`, enforce the final policy.

Changes required:

- strict changed-file unit coverage on `unit-core`
- integration ownership requirement on `boundary-orchestration`
- optional time-boxed warnings for any remaining `legacy-mixed` files

Success criteria:

- a new mixed file cannot land without explicit role classification
- files with strict unit gates are structurally pure enough that coverage remains meaningful
- boundary files pass because real runtime paths are validated, not because wrapper lines were artificially touched

Confirmation gate:

- CI produces separate sections for unit-core and boundary-orchestration failures
- test-only bypass coverage helpers are gone from migrated files

## Success Criteria

### Automated

- `./scripts/run-validate.sh` remains green during each migration phase.
- Unit-core files report strict per-file or changed-file line coverage.
- Boundary files are owned by explicit integration/smoke suites.
- Instrumentation lint reads role classification instead of relying on long exclusion lists for these areas.

### Manual

- Engineers can determine the expected test style from the file path alone.
- Adding a new boundary wrapper no longer pressures authors to add fake unit tests just to satisfy coverage.
- Reviewing TUI/runtime changes becomes simpler because state/render code is not mixed with raw-mode/process wiring.

## Testing Strategy

- Unit:
  - pure parsers, reducers, ordering, answer normalization, frame-model helpers
- Integration:
  - PTY-backed TUI smoke tests for live wrappers and blocking wizards
  - temp-repo or fake-runner tests for worktree reconciliation
- Manual:
  - spot-check real TTY flows after major TUI boundary extractions

## References

- `tools/gardener/src/tui/mod.rs:1-77`
- `tools/gardener/src/tui/backlog.rs:41-214`
- `tools/gardener/src/tui/terminal.rs:26-321`
- `tools/gardener/src/tui/quality.rs:72-249`
- `tools/gardener/src/tui/wizard.rs:43-306`
- `tools/gardener/src/tui/seed_review.rs:241-412`
- `tools/gardener/src/worktree_audit.rs:12-94`
- `tools/gardener/src/runtime/mod.rs:701-752`
- `tools/gardener/src/startup.rs:357-366`
- `tools/gardener/src/startup.rs:1195-1203`
- `tools/gardener/src/triage_interview.rs:93-104`
- `tools/gardener/tests/tui_live_smoke.rs:3-35`
- `tools/gardener/tests/instrumentation_lint.rs:5-43`
- `tools/gardener/tests/instrumentation_lint.rs:81-153`
- `scripts/test-gardener-coverage.sh:5-105`
