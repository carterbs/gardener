## Convention Adherence Assessment

### Repo-Wide Score: 84
Core conventions are clear around runtime entrypoint, pre-commit flow, and Rust linting, and many of them are enforced with executable lint/tests. Compliance appears high in `tools/gardener/`, especially around structured validation and convention-linter tests. The main drag is mismatch between stated lint policy and enforced lint flags, plus missing shell-format/lint enforcement for script-heavy workflows.

### Per-Domain Scores
- runtime-orchestration: 88 - Strong consistency in Rust structure and guardrails (`workspace` clippy lints, pre-commit integration, convention-check tests like command drift/instrumentation/docs sync).
- developer-validation-tooling: 76 - Scripts are organized and tested with fixtures, but shell conventions are not mechanically enforced with `shellcheck`/`shfmt`, so style/robustness can drift over time.

### Key Findings
- Conventions are documented and wired into automation (`.githooks/pre-commit` -> `scripts/run-validate.sh` -> custom linter chain + coverage gate).
- The repo uses convention-focused tests (`command_drift_linter.rs`, `validation_pipeline_docs.rs`, `instrumentation_lint.rs`) to prevent docs/CLI/process drift.
- A policy inconsistency exists: workspace clippy denies `expect_used`, but `scripts/check-no-warnings.sh` explicitly allows it, weakening declared standards.

### Deficiencies
- **[ConventionViolation | P1]** Clippy policy contradiction on `expect_used`
  What: `Cargo.toml` sets `[workspace.lints.clippy].expect_used = "deny"`, but `scripts/check-no-warnings.sh` runs clippy with `-A clippy::expect_used`.
  Agent impact: Agents relying on declared lint rules get false confidence; convention breaches can pass validation, causing review churn and inconsistent remediation.
  Fix: Remove `-A clippy::expect_used` from `scripts/check-no-warnings.sh` (or explicitly scope/test-only allows in code) so enforcement matches declared workspace policy.

- **[MissingTooling | P1]** No shell static-format/lint gate for `scripts/`
  What: Validation pipeline runs several custom script checks, but no `shellcheck` or `shfmt` enforcement is present in active validation commands.
  Agent impact: Script edits become riskier for autonomous agents (quoting/word-splitting/style issues), increasing failed runs and debugging turns.
  Fix: Add `shellcheck` and `shfmt -d` steps to `scripts/run-validate.sh` and pre-commit/CI, with a documented local autofix command.

- **[MissingDocumentation | P2]** Steering docs are minimal versus actual enforced conventions
  What: `AGENTS.md`/`CLAUDE.md` cover runtime entrypoints and commit/worktree policy, but not key style/testing conventions already enforced by tests and scripts.
  Agent impact: Agents must infer standards from code/tests instead of one authoritative spec, slowing onboarding and increasing inconsistent outputs.
  Fix: Expand `AGENTS.md` with a concise “Conventions Contract” section (lint expectations, script validation requirements, doc/CLI drift checks, and where to run canonical validation).