## Test Quality Assessment

### Repo-Wide Score: 93
The sampled `tools/gardener/src/tui.rs` tests are substantial: they cover rendering branches, state normalization, ordering logic, scroll behavior, and wizard key-handling with meaningful assertions. Quality is high overall, but not perfect due to limited coverage of failure/IO paths and some string-fragile UI assertions. This is a strong suite, but not quite a true 100.

### Per-Domain Scores
- runtime-orchestration: 94 - `tui.rs` includes deep unit coverage for state transforms and UI behavior, including edge cases (normalization, truncation, viewport scrolling, wizard branching), with clear test naming and isolation.
- runtime-validation: 90 - broad assertion volume is strong, but quality appears uneven across files (including very low-/zero-assertion contract/lint-style tests), which weakens confidence in behavioral regression detection.
- migration-wiring-fixtures: 88 - fixture-driven validation exists, but fixture domains typically under-test malformed/partial permutations unless explicitly enumerated; risk remains around unmodeled wiring edge cases.

### Key Findings
- The `tui.rs` module has high-quality behavioral tests, not just existence checks, including ordered rendering invariants and input-state transitions.
- Test names are descriptive and map well to expected behavior, which improves maintainability and failure triage.
- The largest remaining risk is around interactive runtime/terminal error paths that are difficult to unit-test and appear lightly covered.

### Deficiencies

- **[CoverageGap | P1] Interactive terminal failure paths are weakly tested**
  - What: Paths in `run_seed_review_wizard`, `run_repo_health_wizard`, `with_live_terminal`, and teardown/error branches are largely unexercised compared to pure render helpers.
  - Agent impact: Autonomous runs can fail on terminal/raw-mode edge cases (resize, IO errors, early exits) with regressions escaping CI until runtime.
  - Fix: Introduce abstraction for event/terminal backends and add deterministic tests for error propagation, cleanup guarantees, and cancel/interrupt flows.

- **[CoverageGap | P2] UI assertions rely heavily on raw string contains**
  - What: Many tests assert `frame.contains("...")` rather than asserting structural invariants (panel boundaries, row selection index behavior, semantic sections).
  - Agent impact: Minor copy or style changes can cause noisy failures, slowing agent iteration and obscuring real behavior regressions.
  - Fix: Add helper assertions that validate semantic regions/ordering and key widgets; keep a smaller set of literal text checks for critical labels only.

- **[FeedbackLoopGap | P2] Assertion quality is uneven across the wider test set**
  - What: Repo metadata shows some files with 0-1 assertions (for example lint/contract tests), creating pockets of shallow verification.
  - Agent impact: Agents may get false confidence from passing checks while behavioral bugs in adjacent flows remain undetected.
  - Fix: Raise minimum expectations for low-assertion tests (multi-assert behavioral checks, negative-case assertions, failure-message validation) and enforce via test quality linting.