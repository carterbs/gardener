## Risk Exposure Assessment

### Repo-Wide Score: 71
Risk is moderate, not low: the sampled runtime code shows very high complexity concentration, and 29/98 source files are untested (including several runtime entry/boundary modules). `tui.rs` is heavily tested for render/key-path behavior, which offsets some risk, but coverage is uneven across other critical orchestration paths.

### Per-Domain Scores
- runtime-orchestration: 68 - Elevated exposure from very large/high-branch files (`tui.rs`, `worker_pool.rs`, `backlog_store.rs`, `startup.rs`) plus notable untested runtime boundaries (`main.rs`, `phase_cli.rs`, `config.rs`, `merge_loop.rs`, `pr_audit.rs`).
- developer-validation-tooling: 83 - Lower complexity and smaller surface area; primary risk is fixture/script drift rather than deep branching runtime failures.

### Key Findings
- Complexity is highly concentrated in a few core runtime files, increasing regression blast radius for small edits.
- Test coverage is strong in sampled `tui.rs` logic paths, but untested boundary modules create orchestration failure risk.
- Debt markers are clustered (especially in quality/debt-related modules), signaling deferred cleanup around quality control logic itself.

### Deficiencies

- **CoverageGap | P0** Untested runtime boundaries in orchestration flow
  - What: Multiple runtime boundary files are untested (`tools/gardener/src/main.rs`, `phase_cli.rs`, `config.rs`, `git_phase.rs`, `merge_loop.rs`, `pr_audit.rs`, `plan_phase.rs`).
  - Agent impact: Agents can pass unit checks but still fail at real startup/phase transitions, causing failed runs and expensive reruns.
  - Fix: Add focused integration tests for CLI entrypoints and phase handoffs (happy path + failure path), starting with `main.rs`/`phase_cli.rs`/`merge_loop.rs`.

- **FeedbackLoopGap | P1** Live TUI terminal paths are less exercised than test-backend rendering
  - What: `tui.rs` tests heavily validate `TestBackend` rendering/state logic, but live terminal lifecycle paths (`with_live_terminal`, raw mode, resize/autoresize, teardown) have limited direct verification.
  - Agent impact: Interactive failures appear only at runtime (stuck terminal state, redraw/resize issues), creating flaky manual recovery and slower autonomous loops.
  - Fix: Add integration tests around live terminal setup/teardown and resize behavior, with fault-injection for I/O errors.

- **ConventionViolation | P1** Oversized “god files” in critical runtime modules
  - What: Very large modules (`tui.rs` ~3647 LOC, `worker_pool.rs` ~2354, `backlog_store.rs` ~2597) mix UI/state parsing/control flow.
  - Agent impact: Small agent changes touch dense logic with many branches, increasing unintended side effects and review burden.
  - Fix: Incrementally split by responsibility (state parsing, rendering, input handling, terminal lifecycle) and enforce max-module-size/complexity guardrails in CI.

- **ObservabilityGap | P2** Panic-based UI rendering failure paths reduce diagnosability
  - What: Several render helpers use `panic!`/`unwrap_or_else(panic!)` patterns in rendering functions.
  - Agent impact: On rendering faults, agents get abrupt crashes instead of structured errors, making root-cause analysis slower and less reliable.
  - Fix: Convert panic paths to `Result<_, GardenerError>` propagation with structured logging fields (stage, terminal size, panel).