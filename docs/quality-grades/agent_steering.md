## Agent Steering Assessment

### Repo-Wide Score: 61
The steering docs are very concise and contain concrete runtime/commit/worktree directives, which is high-signal. However, they miss two major effectiveness drivers: architecture pointers and explicit test/validation commands. Coverage is heavily skewed to runtime execution and does not guide agents through validation or fixture domains.

### Per-Domain Scores
- runtime-orchestration: 78 - `AGENTS.md` gives clear Rust-first direction and exact runtime commands, plus strong commit/worktree constraints.
- runtime-validation: 38 - No explicit test commands (`cargo test` targets, integration/contract/lint entrypoints) or guidance on when/how to run validation suites.
- migration-wiring-fixtures: 27 - No mention of fixture usage, migration wiring checks, or commands for verifying pass/fail fixture behavior.

### Key Findings
- Strong specificity for runtime execution: exact `cargo run -p gardener --bin gardener -- ...` commands reduce ambiguity.
- Excellent signal-to-noise ratio (21 total lines across both files), with minimal boilerplate and clean progressive disclosure (`CLAUDE.md` -> `AGENTS.md`).
- Critical coverage gaps: missing architecture map and test/verification commands substantially limit autonomous agent reliability.

### Deficiencies

- **MissingTooling | P1** Missing test and verification command matrix
  - What: `AGENTS.md` defines run commands but no concrete test/build/lint commands for `tools/gardener/tests/` or repo-level validation flows.
  - Agent impact: Agents guess validation steps, causing missed regressions, extra turns, or incorrect “done” states after code changes.
  - Fix: Add a compact “Verification” section with exact commands (unit, integration, fixture checks, lint/format) and when each is required.

- **CoverageGap | P1** No architecture pointers for core modules
  - What: Steering docs do not map key runtime areas in `tools/gardener/src/` (orchestration, TUI, worker lifecycle, git/worktree ops, quality pipeline).
  - Agent impact: Slower navigation and higher risk of editing wrong components, increasing failed attempts and patch churn.
  - Fix: Add a short “Architecture Pointers” section listing major modules/paths and their responsibilities (1 line each).

- **CoverageGap | P2** Fixture/migration domain not represented
  - What: No guidance references `scripts/fixtures/check-migrations-wired/` or how fixture-driven migration wiring validation is expected to run.
  - Agent impact: Agents underuse fixture checks and can miss migration wiring regressions tied to backlog-store behavior.
  - Fix: Add a “Fixtures” subsection with fixture path purpose and exact command(s) to run pass/fail wiring checks.