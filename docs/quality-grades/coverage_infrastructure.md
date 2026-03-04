## Coverage Infrastructure Assessment

### Repo-Wide Score: 74
Coverage is actively enforced with `cargo llvm-cov` in CI and local validation (`.github/workflows/ci.yml`, `.github/workflows/gardener-coverage.yml`, `scripts/test-gardener-coverage.sh`) with a numeric line gate (`COVERAGE_MIN_LINE`, default 90). It is not top-tier because reporting is summary-only (no artifact publishing, PR annotations, or badge integration) and coverage scope is reduced by explicit path exclusions (`scripts/coverage-ignore-manifest.txt`).

### Per-Domain Scores
- runtime-orchestration: 81 - `tools/gardener/src/` is covered by a real, failing CI gate via `cargo llvm-cov`, but confidence is reduced by ignore-manifest exclusions and lack of per-change visibility.
- integration-and-contract-testing: 62 - `tools/gardener/tests/` is exercised in CI through `--all-targets`, but there is no domain-specific threshold/reporting surface for integration/contract layers.
- developer-automation-and-fixtures: 28 - `scripts/` and fixture logic are validated functionally, but no script-level coverage tooling/thresholds (e.g., `kcov`) are wired into CI.

### Key Findings
- Coverage gating is real and enforced in multiple workflows using `cargo llvm-cov` plus a numeric fail condition.
- Coverage observability is weak: no LCOV/HTML artifact upload, no coverage service integration, and no README badge.
- Scope exclusions in `scripts/coverage-ignore-manifest.txt` materially weaken denominator trust for runtime coverage.

### Deficiencies

- **[ObservabilityGap | P1]** Coverage results are log-only
  - What: `scripts/test-gardener-coverage.sh` runs `cargo llvm-cov --summary-only`; workflows do not publish `lcov.info`/HTML or PR annotations.
  - Agent impact: Agents and reviewers must parse raw logs, making regressions harder to detect and slowing remediation loops.
  - Fix: Emit machine-readable coverage outputs (`--lcov`/HTML), upload artifacts in GitHub Actions, and add PR annotation/reporting integration.

- **[CoverageGap | P1]** Runtime coverage denominator is narrowed by ignore manifest
  - What: `scripts/coverage-ignore-manifest.txt` excludes many `tools/gardener/src/**` paths from coverage math.
  - Agent impact: Green coverage gates can overstate confidence, increasing risk of missed regressions in orchestrator-critical code.
  - Fix: Audit and minimize exclusions, require justification for new ignores, and enforce policy checks for high-risk module exclusions.

- **[MissingTooling | P1]** No dedicated coverage tooling for scripts/fixtures
  - What: `scripts/` and `scripts/fixtures/check-migrations-wired/` are executed by validation, but no script/fixture coverage tool or threshold is configured.
  - Agent impact: Shell/fixture branch regressions can pass CI unnoticed, reducing reliability of automation infrastructure that agents depend on.
  - Fix: Add script coverage tooling (for example `kcov`) for high-impact scripts, set minimum thresholds, and gate in CI.