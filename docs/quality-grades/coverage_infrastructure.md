## Coverage Infrastructure Assessment

### Repo-Wide Score: 76
Coverage is actively measured and enforced for Rust via `cargo llvm-cov` in both CI workflows and local validation (`scripts/test-gardener-coverage.sh` with `COVERAGE_MIN_LINE`, default 90). The score is capped because reporting is terminal-only (`--summary-only`), with no artifact publishing, PR annotations, or badge/trend visibility.

### Per-Domain Scores
- runtime-orchestration: 80 - `tools/gardener/src/` is covered by a real CI gate (`cargo llvm-cov`, fail-below-threshold), but enforcement scope is reduced by `scripts/coverage-ignore-manifest.txt` exclusions and lacks rich reporting.
- runtime-validation: 56 - Validation runs in CI and coverage gate execution is tested, but there is no domain-specific threshold/reporting for `tools/gardener/tests/` itself.
- migration-wiring-fixtures: 22 - Fixture behavior is tested through shell fixture runs, but there is no coverage instrumentation or threshold for fixture/script paths.

### Key Findings
- CI has explicit coverage enforcement (`.github/workflows/ci.yml`, `.github/workflows/gardener-coverage.yml` -> `./scripts/test-gardener-coverage.sh`).
- The gate is numeric and failing (`COVERAGE_MIN_LINE` default `90`, parse `TOTAL`, exit non-zero below threshold).
- Coverage visibility is weak: no LCOV/HTML artifact upload, no Codecov/Coveralls integration, and no README badge.

### Deficiencies

- **[CoverageGap | P1]** Coverage gate excludes many runtime files
  - What: `scripts/test-gardener-coverage.sh` applies `--ignore-filename-regex` from `scripts/coverage-ignore-manifest.txt`, removing substantial `tools/gardener/src/**` paths from denominator math.
  - Agent impact: Agents can change excluded runtime code and still pass the gate, increasing missed-regression risk and false confidence in autonomous runs.
  - Fix: Shrink the ignore manifest to true non-actionable paths only; add a policy test that fails if high-risk modules are newly excluded.

- **[ObservabilityGap | P1]** No published coverage artifacts or PR-level visibility
  - What: Workflows run coverage checks but only print summary text; there is no `lcov.info`/HTML upload, no PR annotation, and no external reporting integration.
  - Agent impact: Agents/reviewers must parse raw logs manually, slowing triage and making coverage regressions harder to spot quickly.
  - Fix: Emit machine-readable output (LCOV/JSON), upload artifacts in CI, and optionally wire Codecov/Coveralls or PR comments for change-level visibility.

- **[MissingTooling | P1]** No coverage tooling for fixture/script domain
  - What: `scripts/fixtures/check-migrations-wired/**` and related shell checks are behavior-tested, but shell coverage tooling (e.g., `kcov`/`bashcov`) is not configured or gated.
  - Agent impact: Script-path regressions can survive because execution success does not reveal untested branches in fixture validation logic.
  - Fix: Add shell/script coverage collection in CI for critical scripts and enforce a minimum threshold for the migration-wiring fixture workflow.