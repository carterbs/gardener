# Coverage Boundary Separation Execution Spec

## Objective

Do the separation in one shot.

The repository should end this change with a durable testability architecture:

- pure runtime logic lives in files where unit coverage is meaningful
- side-effect adapters live in files where integration and instrumentation are the primary signal
- CI can determine the required gate mechanically
- no file sits in an ambiguous middle state

This is not a phased migration plan. This is the target shape for one change set that rewires code layout and validation together.

## Operating Principles

The implementation should follow these rules:

- enforce invariants mechanically, not by reviewer memory
- optimize for agent legibility and repository-local truth
- keep documentation as the system of record, but back it with tests and lints
- prefer rigid boundaries with local implementation freedom inside those boundaries
- do not create transitional buckets that become permanent

Those principles align with the repo direction and with `docs/references/codex-agent-team-article.md`: the foundation must be explicit, enforced, and legible to future agent runs.

## Non-Negotiable End State

When this lands, every Rust runtime file under the affected areas must be in exactly one of these classes:

1. `unit-core`
   Pure parsing, normalization, reducers, ordering, formatting, render-model building, and deterministic state transitions.

2. `boundary-orchestration`
   Terminal lifecycle, PTY handling, process spawning, cwd/env lookup, git/worktree calls, blocking loops, logging dispatch, and other side-effect adapters.

There is no `legacy-mixed` class.

If a file cannot be clearly classified, the refactor is incomplete.

Enforcement scope is all Rust source files under `tools/gardener/src/**` except paths explicitly listed in a checked-in allowlist.

CI must fail if any in-scope file is missing a manifest entry.

## Why This Exists

The current TUI and worktree code mixes logic and side effects in ways that make the coverage gate produce bad incentives:

- `tools/gardener/src/tui/terminal.rs` mixes terminal lifecycle with pure frame rendering
- `tools/gardener/src/tui/quality.rs` mixes live drawing entrypoints with pure render/data shaping
- `tools/gardener/src/tui/wizard.rs` mixes answer normalization and reducers with a blocking TTY runner
- `tools/gardener/src/tui/seed_review.rs` mixes pure prompt/state transition logic with a blocking TTY runner
- `tools/gardener/src/worktree_audit.rs` mixes summary logic with cwd/process/git/logging side effects

That mixed structure is why wrapper-line coverage became a problem. The fix is not weaker review. The fix is better code boundaries.

## One-Shot Structural Rewrite

This change should split the current mixed files into stable role-based modules.

### TUI target layout

- `tools/gardener/src/tui/state/`
  - pure reducers and state machines
- `tools/gardener/src/tui/views/`
  - pure ratatui frame builders, formatting helpers, and render-model shaping
- `tools/gardener/src/tui/live/`
  - real terminal lifecycle, blocking loops, resize handling, and live wrapper entrypoints

Expected moves:

- move `WizardState`, `WizardAction`, answer normalization, and key-handling reducers out of `tools/gardener/src/tui/wizard.rs`
- move `SeedReviewState`, `InputMode`, reducer helpers, and finalization logic out of `tools/gardener/src/tui/seed_review.rs`
- move pure seeding/shutdown frame builders out of `tools/gardener/src/tui/terminal.rs`
- move pure quality intro/grading frame builders and render helpers out of `tools/gardener/src/tui/quality.rs`
- keep `with_live_terminal`, live draw wrappers, `run_repo_health_wizard`, and `run_seed_review_wizard` in `tui/live/`

### Worktree target layout

- `tools/gardener/src/worktree_audit/model.rs`
  - summary and result types
- `tools/gardener/src/worktree_audit/logic.rs`
  - pure classification and result shaping
- `tools/gardener/src/worktree_audit/live.rs`
  - cwd lookup, runner creation, git worktree interaction, prune/list calls, logging, and top-level orchestration

### Rule for call direction

- `unit-core` may not depend on `boundary-orchestration`
- `views/` may depend on `state/` and other pure helpers
- `live/` may depend on `state/`, `views/`, and boundary adapters
- worktree `logic.rs` may not depend on process, fs, env, git invocation, or logging

## Mechanical Enforcement

Path layout alone is not enough. The repository must enforce purity.

### Purity contract for `unit-core`

Files classified as `unit-core` must not:

- import terminal or PTY handling crates
- spawn processes
- read cwd or environment state
- touch the filesystem
- call git or worktree clients
- emit logs or telemetry side effects
- access runtime singletons

This is enforced by a manifest-driven structural test that parses Rust source and rejects:

- imports from banned crates or modules for `unit-core`
- calls to banned APIs, including `std::process`, `std::fs`, env or cwd access, logging sinks, and terminal or PTY APIs
- dependency edges from `unit-core` modules to `boundary-orchestration` modules

String-matching heuristics are not sufficient.

### Boundary contract for `boundary-orchestration`

Boundary files are allowed to do side effects, but they must be thin:

- they orchestrate environment setup, IO, and lifecycle
- they delegate deterministic decision making to `unit-core`
- they are owned by explicit integration tests

Boundary files may coordinate IO, lifecycle, and adapter translation only.

They must not define:

- domain reducers
- ranking or ordering policy
- answer normalization
- render-policy branching except for trivial adapter-local mapping

Any helper with deterministic business behavior and no side effects belongs in `unit-core`.

## Validation Model

This repo keeps the global coverage gate and adds role-aware enforcement.

### Gate 1: Repository floor

Keep the repository-wide `cargo llvm-cov` threshold in `scripts/test-gardener-coverage.sh` as the coarse floor for the crate.

### Gate 2: Strict unit-core coverage

Changed files in `unit-core` paths must meet strict per-file line coverage in CI.

Policy:

- required threshold: `>= 90%` line coverage
- evaluated on changed files in `unit-core`
- uncovered lines are reported by file
- no test-only bypass branches are allowed in `unit-core`

Long-term invariant:

- every manifest-classified `unit-core` file should remain coverable to the same threshold

Operational rules:

- diff base is `git merge-base HEAD origin/main`
- renames are treated as the new path and must satisfy the gate under the new classification
- extracted files are treated as new files and must meet the gate immediately
- moved uncovered code does not get grandfathered

### Gate 3: Boundary ownership and execution

Every `boundary-orchestration` file must have at least one required owning integration test target.

This is not optional.

The repo should add a checked-in manifest, for example `tools/gardener/testability-boundaries.toml`, that records for every affected file:

- role: `unit-core` or `boundary-orchestration`
- owning test target(s)
- for boundary files, whether the owner is PTY-backed, temp-repo-backed, or both
- instrumentation requirement for the file

Example shape:

```toml
[[file]]
path = "tools/gardener/src/tui/live/example.rs"
role = "boundary-orchestration"
owning_tests = ["tui_live_smoke"]
boundary_mode = ["pty"]
instrumentation = "required"
```

CI must fail when:

- a file has no classification
- a boundary file has no owning test target
- a changed boundary file’s owning test target did not run
- a changed boundary file’s owning test target ran but did not produce execution evidence for that file
- a `unit-core` file violates purity rules

A boundary file is considered owned only if its declared owning test target both runs and produces execution evidence for that file in CI.

Acceptable evidence is either:

- non-zero line execution in coverage output for the owned file
- a required runtime trace or assertion emitted from the real boundary path

### Gate 4: Instrumentation role awareness

`tools/gardener/tests/instrumentation_lint.rs` should read the same role manifest.

Policy:

- `boundary-orchestration` files that own runtime observability remain subject to instrumentation expectations
- `unit-core` files are exempt from instrumentation thresholds unless they intentionally define telemetry payload shaping as pure data

## Required Integration Surfaces

Boundary validation should be real enough to catch wiring failures and narrow enough to stay stable.

### TUI boundaries

Use PTY-backed integration tests to own:

- live terminal wrappers
- blocking wizard loops
- resize/lifecycle behavior that cannot be validated meaningfully in unit tests

### Worktree boundaries

Use a hybrid strategy:

- real temp-repo integration tests for the happy path and the important failure path of `git worktree` behavior
- fake seams only for rare error injection that real git cannot produce deterministically

The worktree boundary should not be validated only through a fake runner.

## Concrete File Classification

These files are already obvious `unit-core` or should become so after the rewrite:

- `tools/gardener/src/tui/backlog.rs`
- extracted files under `tools/gardener/src/tui/state/**`
- extracted pure view files under `tools/gardener/src/tui/views/**`
- extracted pure worktree logic under `tools/gardener/src/worktree_audit/logic.rs`

These files are `boundary-orchestration` after the rewrite:

- files under `tools/gardener/src/tui/live/**`
- `tools/gardener/src/worktree_audit/live.rs`

The current mixed files should not survive unchanged.

## Success Criteria

This work is complete only if all of the following are true in one merged change:

- the mixed TUI and worktree files are physically split into pure and boundary modules
- every affected file is classified in a checked-in role manifest
- purity rules are mechanically enforced for `unit-core`
- boundary ownership is mechanically enforced for `boundary-orchestration`
- changed `unit-core` files are held to strict per-file coverage
- boundary files are validated by explicit owning integration tests
- existing test-only bypass hooks in live adapter files are removed if the new boundary tests cover their intent
- `./scripts/run-validate.sh` passes

## Developer Experience Guardrails

The new foundation should not quietly make the repo worse.

Track and enforce:

- CI fails if the boundary suite runtime increases by more than 10% from the checked-in baseline without updating the baseline in the same change with justification
- CI fails if the rolling 14-day flaky retry rate for PTY-backed and temp-repo-backed boundary tests exceeds 2%

If the boundary suite exceeds either threshold, the solution is to improve the owning boundary tests, not to weaken the architectural split.

## Execution Order Inside the One-Shot Change

This is one change set, but the internal order should still be deliberate:

1. extract pure state and view logic out of mixed TUI files
2. extract worktree pure logic out of `worktree_audit.rs`
3. create stable live adapter files with thin orchestration only
4. add the role manifest
5. enforce purity and boundary ownership mechanically
6. wire coverage and instrumentation gates to the manifest
7. remove obsolete bypasses and update tests to match the new ownership model

That order exists to keep the implementation coherent inside one branch, not to justify a future phased rollout.

## Foundation Standard

The goal is not just to get the current gate green.

The goal is to leave the repo with a structure that future agents can read, trust, and extend without recreating this mess:

- code paths should tell you what kind of test is expected
- validation should be derived from explicit repo metadata, not tribal knowledge
- architectural drift should fail mechanically before it spreads

If this lands correctly, the coverage gate stops being a blunt instrument and becomes part of a durable architecture.
