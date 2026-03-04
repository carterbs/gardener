## Local Feedback Loop Assessment

### Repo-Wide Score: 80
Local validation is well-defined and CI-reproducible: developers can run the same validation path as CI through `./scripts/run-validate.sh`, and the workflow is documented. The main drag is loop speed, because default commit-time validation includes full coverage gating (`cargo llvm-cov --all-targets`) and there is no first-class test/lint watch loop.

### Per-Domain Scores
- runtime-orchestration: 78 - Strong command parity and clear entrypoints for full validation, but edit-validate cycles are heavier than needed for small Rust changes.
- runtime-validation: 86 - Extensive integration/lint/fixture tests under `tools/gardener/tests` with deterministic shell harnesses and clear remediation paths.
- migration-wiring-fixtures: 75 - Fixture-backed script validation exists and is wired into the main validation flow, but this area lacks an independently documented quick-run command for tight iteration.

### Key Findings
- CI and local checks are aligned: both workflows run the same validation scripts (`./scripts/run-validate.sh` and `./scripts/test-gardener-coverage.sh`).
- Pre-commit is enforced and deterministic via [`.githooks/pre-commit`](/Users/bradcarter/Documents/Dev/gardener/.claude/worktrees/quality-grading-tui/.githooks/pre-commit), including rustfmt and full validation.
- Fast-loop ergonomics are weaker: no root `Makefile`/`justfile`, no test/lint watch mode, and coverage runs in the default validation path.

### Deficiencies

- **[FeedbackLoopGap | P1] Heavy default validation path**
  - What: [`.githooks/pre-commit`](/Users/bradcarter/Documents/Dev/gardener/.claude/worktrees/quality-grading-tui/.githooks/pre-commit) always calls [`scripts/run-validate.sh`](/Users/bradcarter/Documents/Dev/gardener/.claude/worktrees/quality-grading-tui/scripts/run-validate.sh), which ends with [`scripts/test-gardener-coverage.sh`](/Users/bradcarter/Documents/Dev/gardener/.claude/worktrees/quality-grading-tui/scripts/test-gardener-coverage.sh) (`cargo llvm-cov -p gardener --all-targets`).
  - Agent impact: Small edits pay full coverage cost, slowing iteration and reducing the number of fix/verify cycles an autonomous agent can complete per session.
  - Fix: Split into `validate-fast` (fmt+clippy+targeted tests) and `validate-full` (adds coverage), keep full in CI, and point pre-commit to fast by default.

- **[MissingTooling | P2] No unified local task runner surface**
  - What: There is no root `Makefile` or `justfile`; validation logic is spread across scripts and docs.
  - Agent impact: Command discovery overhead increases and agents are more likely to run partial checks or wrong sequences.
  - Fix: Add a thin `justfile`/`Makefile` with canonical targets (`test`, `lint`, `validate-fast`, `validate-full`, `coverage`, `hooks`) that delegate to existing scripts.

- **[FeedbackLoopGap | P2] Missing first-class test/lint watch workflow**
  - What: The repo has [`scripts/watch-otel-logs.sh`](/Users/bradcarter/Documents/Dev/gardener/.claude/worktrees/quality-grading-tui/scripts/watch-otel-logs.sh) for logs, but no documented watch loop for Rust tests/lints.
  - Agent impact: Agents must rerun one-shot commands repeatedly, which lengthens turnaround during iterative debugging/refactoring.
  - Fix: Add documented watch commands (for example `cargo watch -x 'test -p gardener --all-targets'` and `cargo watch -x 'clippy -p gardener --all-targets -- -D warnings'`) in README/workflow docs.

- **[MissingDocumentation | P2] Fast-path validation commands are under-documented**
  - What: [`README.md`](/Users/bradcarter/Documents/Dev/gardener/.claude/worktrees/quality-grading-tui/README.md) primarily documents full-suite commands, with limited guidance on targeted per-file/per-test validation for quick checks.
  - Agent impact: Agents default to expensive full runs more often, increasing latency and compute waste.
  - Fix: Document a “quick loop” section with concrete targeted commands (single test file, single test name, clippy on changed crate) and expected use cases.