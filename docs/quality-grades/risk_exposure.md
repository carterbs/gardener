## Risk Exposure Assessment

### Repo-Wide Score: 61
Risk is elevated overall: core runtime files are very large and deeply branched, and 29/98 source files are untested, including orchestration-critical paths. The sampled `tui.rs` has strong unit coverage, but its size and statefulness still create regression surface area that tests do not fully close.

### Per-Domain Scores
- runtime-orchestration: 54 - Highest complexity concentration (`tui.rs`, `backlog_store.rs`, `worker_pool.rs`, `startup.rs`) plus multiple untested runtime files in control-flow boundaries raises bug/regression exposure.
- runtime-validation: 76 - Test suite is substantial and catches many UI/state paths, but coverage is uneven against high-risk orchestration and terminal/error-edge behavior.
- migration-wiring-fixtures: 72 - Simple fixture scope limits blast radius, but fixture code is untested and can silently drift from real migration wiring expectations.

### Key Findings
- `tools/gardener/src/tui.rs` is heavily tested but still a monolithic, high-branching control surface with global thread-local state and multiple rendering modes.
- Critical runtime files listed as untested (`git_phase.rs`, `plan_phase.rs`, `merge_loop.rs`, `phase_cli.rs`, `quality_scoring.rs`, `config.rs`) create disproportionate regression risk versus their role.
- Error boundaries in rendering paths still include `panic!`-style failure behavior, increasing hard-failure risk under terminal/runtime edge conditions.

### Deficiencies

- **CoverageGap | P0** Untested orchestration paths in critical runtime modules
  - What: Key files in `tools/gardener/src/` (including `git_phase.rs`, `plan_phase.rs`, `merge_loop.rs`, `phase_cli.rs`, `quality_scoring.rs`, `config.rs`) are reported untested.
  - Agent impact: Autonomous runs can fail late in phase transitions/merge flows, causing wasted cycles and missed regressions in the exact paths agents exercise most.
  - Fix: Add focused integration tests per phase boundary (claim → plan → do → merge), plus contract tests for config parsing and scoring outputs.

- **ConventionViolation | P1** Monolithic TUI module with mixed responsibilities
  - What: `tools/gardener/src/tui.rs` combines rendering, input handling, wizard logic, state normalization, scroll state, and terminal lifecycle in one very large file.
  - Agent impact: Changes are harder to localize safely; agents must touch broad surfaces, increasing accidental regressions and review complexity.
  - Fix: Split into submodules (`render_dashboard`, `render_report`, `wizard`, `terminal_lifecycle`, `state_formatting`) and enforce max file/function complexity thresholds in CI.

- **ObservabilityGap | P1** Panic-based failure in render/test paths
  - What: Multiple render helpers use `panic!` on terminal initialization/draw failure rather than propagating structured errors.
  - Agent impact: Failures become abrupt and less diagnosable, reducing agent ability to recover, retry, or provide actionable remediation.
  - Fix: Replace panic branches with `Result` propagation and add structured error context (mode, dimensions, terminal state) to logs.

- **FeedbackLoopGap | P2** Debt clusters around quality subsystem
  - What: Debt markers are concentrated in `quality_debt_scanner.rs` and related quality files, indicating known cleanup/consistency gaps near scoring logic.
  - Agent impact: Agents spend extra turns reconciling ambiguous behavior in quality reporting, slowing iteration and increasing inconsistent outputs.
  - Fix: Burn down debt markers in quality pipeline files first, then add lint/check to fail on new `TODO/FIXME` in scoring and assessment modules.