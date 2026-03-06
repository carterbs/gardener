## Mechanical Guardrails Assessment

### Repo-Wide Score: 84
Guardrails are strong: CI runs `./scripts/run-validate.sh`, pre-commit runs the same validation path, and Rust linting/coverage are hard-fail (`cargo clippy -D warnings`, `COVERAGE_MIN_LINE` gate in `scripts/test-gardener-coverage.sh`). The main gaps are missing security/dependency scanning and incomplete formatting/coverage observability enforcement in CI.

### Per-Domain Scores
- runtime-orchestration: 86 - `tools/gardener/src/` is protected by workspace clippy denies (`Cargo.toml`) plus CI/pre-commit validation and coverage gating.
- integration-and-contract-testing: 82 - `tools/gardener/tests/` is exercised through coverage runs and targeted lint/contract tests, but coverage reporting is summary-only and excludes configured paths.
- developer-automation-and-fixtures: 84 - `scripts/` has substantial self-checks (`run-script-lint-fixture-tests.sh`, migration/skills/doc linters), but script/supply-chain security tooling is limited.

### Key Findings
- CI has two active workflows (`.github/workflows/ci.yml`, `.github/workflows/gardener-coverage.yml`) that fail builds on validation/coverage regressions.
- Pre-commit is wired (`.githooks/pre-commit`) and runs repository-wide validation, not just formatting.
- Coverage is quantitatively enforced (`scripts/test-gardener-coverage.sh`) with a default 85% line threshold.

### Deficiencies

- **[MissingTooling | P1]** No dependency/security scanning gate
  - What: No `cargo deny`, `cargo audit`, SCA workflow, or equivalent security check appears in `.github/workflows/*.yml` or validation scripts.
  - Agent impact: Vulnerable or policy-violating dependencies can land undetected, causing late-cycle break/fix work and riskier autonomous updates.
  - Fix: Add a CI stage (and pre-commit integration where practical) for `cargo audit` and/or `cargo deny check` with fail-on-findings policy.

- **[CoverageGap | P1]** Coverage gate excludes files via ignore manifest
  - What: `scripts/test-gardener-coverage.sh` applies `--ignore-filename-regex` from `scripts/coverage-ignore-manifest.txt`.
  - Agent impact: Agents can change excluded runtime code without moving the coverage gate, increasing missed-regression probability.
  - Fix: Tighten the ignore manifest to minimal justified exclusions and add a policy check that blocks new exclusions without review.

- **[FeedbackLoopGap | P2]** Coverage feedback is log-only
  - What: Workflows run coverage but do not publish HTML/LCOV artifacts or PR annotations (`.github/workflows/gardener-coverage.yml`).
  - Agent impact: Regression triage is slower because agents/reviewers must parse raw logs instead of targeted diffs/artifacts.
  - Fix: Emit LCOV/HTML (`cargo llvm-cov --lcov/--html`) and upload artifacts; optionally add PR comments for changed-file coverage.

- **[ConventionViolation | P2]** Formatting is not explicitly CI-enforced
  - What: Pre-commit formats staged Rust files, but CI does not run an explicit `cargo fmt --all --check` step in workflows.
  - Agent impact: If hooks are bypassed/misconfigured, style drift can merge and create noisy follow-up churn for autonomous edits.
  - Fix: Add `cargo fmt --all --check` to `scripts/run-validate.sh` (or workflow steps) as a hard CI gate.
