## Coverage Infrastructure Assessment

### Repo-Wide Score: 64
Coverage is measured and gated for the Rust `gardener` package via `cargo llvm-cov` in both CI and pre-commit (`scripts/test-gardener-coverage.sh`, `.github/workflows/ci.yml`, `.github/workflows/gardener-coverage.yml`). However, reporting is limited to console summaries, there is no published coverage artifact/badge, and coverage enforcement does not extend to the `scripts/` domain.

### Per-Domain Scores
- runtime-orchestration: 78 - Coverage is actively measured and threshold-enforced in CI/pre-commit, but visibility is low (no uploaded reports) and ignore-based filtering reduces confidence in full-path enforcement.
- developer-validation-tooling: 22 - Validation scripts are tested functionally, but there is no script-level coverage instrumentation, no script coverage report, and no threshold gate for this domain.

### Key Findings
- Rust coverage gating is wired and enforced with a numeric minimum (`COVERAGE_MIN_LINE`, default 90) through CI and local validation.
- Coverage output is `--summary-only` and not persisted as artifacts; there is no Codecov/Coveralls/badge integration.
- `scripts/coverage-ignore-manifest.txt` excludes many Rust paths, and `scripts/` tooling itself has no measurable coverage framework.

### Deficiencies

- **CoverageGap | P1** Single-surface coverage gate with broad ignore scope
  - What: Coverage enforcement is centralized in `scripts/test-gardener-coverage.sh`, but `scripts/coverage-ignore-manifest.txt` excludes a large set of `tools/gardener/src/**` paths (including major runtime areas).
  - Agent impact: Agents can pass the coverage gate while modifying behavior in excluded files, increasing missed-regression risk and false confidence in autonomous merges.
  - Fix: Reduce ignore list to exceptional cases only, add path-based minimums (e.g., runtime core, worker, CLI), and fail CI if any protected path drops below its threshold.

- **ObservabilityGap | P1** No durable coverage reporting/trend surface
  - What: CI runs coverage but only prints terminal summary output; no LCOV/HTML artifact upload and no external reporting/badge.
  - Agent impact: Agents cannot quickly inspect file/module deltas after failures, increasing diagnosis time and retry turns.
  - Fix: Generate LCOV/HTML in CI, upload artifacts, and optionally wire Codecov/Coveralls for PR diff comments and repository trend visibility.

- **MissingTooling | P1** No coverage infrastructure for `scripts/` domain
  - What: `scripts/` has guardrail tests, but no shell coverage tooling (e.g., `kcov`/`bashcov`) and no coverage threshold in CI.
  - Agent impact: Automation logic regressions in validation scripts are harder to detect quantitatively, making autonomous refactors riskier.
  - Fix: Add script coverage tooling for key shell scripts, publish a script-domain report, and enforce a minimum threshold in CI alongside Rust coverage.