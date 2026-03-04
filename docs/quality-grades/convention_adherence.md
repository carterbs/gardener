## Convention Adherence Assessment

### Repo-Wide Score: 74
Conventions are clearly defined and heavily instrumented (steering docs, workspace lints, custom linters, pre-commit workflow), but current compliance is not fully green. Spot checks and validation runs show active drift in runtime code and a brittle validation prerequisite around quality-grade artifacts.

### Per-Domain Scores
- runtime-orchestration: 68 - Core Rust runtime code is mostly consistent in structure and naming, but current Clippy gate failures in `src/` show convention violations in active paths.
- runtime-validation: 84 - Test/linter coverage is strong and convention-aware (`command_drift_linter`, `quality_dimension_linter`, Clippy-config tests), though some checks are brittle and policy alignment is imperfect.
- migration-wiring-fixtures: 93 - Fixtures are clean, minimal, and directly aligned with `check-migrations-wired.sh` pass/fail expectations.

### Key Findings
- Conventions are explicit and enforceable: `AGENTS.md`, `docs/conventions/workflow.md`, `.githooks/pre-commit`, and `scripts/run-validate.sh` are aligned on Rust-first runtime and validation flow.
- The repo uses strong custom convention guards (CLI command drift, quality-dimension sync, migration wiring fixture tests), which improves consistency for autonomous changes.
- Current enforcement state is not clean: `./scripts/check-no-warnings.sh` fails on multiple runtime files, and `./scripts/run-script-lint-fixture-tests.sh` fails due missing `docs/quality-grades.md`.

### Deficiencies

- **ConventionViolation | P1** Runtime lint conformance currently broken
  - What: `./scripts/check-no-warnings.sh` fails with concrete issues in `tools/gardener/src/tui.rs:1689`, `tools/gardener/src/protocol.rs:182`, `tools/gardener/src/seeding.rs:133`, and `tools/gardener/src/startup.rs:1447`.
  - Agent impact: Autonomous agents will repeatedly fail pre-commit/validation loops, wasting turns on avoidable style debt before functional regressions can be evaluated.
  - Fix: Resolve the listed Clippy findings immediately, then require `scripts/check-no-warnings.sh` as a fast local gate before broader validation.

- **MissingTooling | P1** Lint policy mismatch around `expect_used`
  - What: Workspace policy denies `expect_used` in `Cargo.toml`, but `scripts/check-no-warnings.sh:8-9` explicitly passes `-A clippy::expect_used`.
  - Agent impact: Mixed signals make automated edits inconsistent; agents cannot reliably infer whether `expect()` usage is a violation or accepted practice.
  - Fix: Unify policy by either removing `-A clippy::expect_used` or scoping/ documenting explicit exceptions (for test-only targets) and enforcing that policy deterministically.

- **FeedbackLoopGap | P1** Validation depends on missing generated artifact
  - What: Fixture-script validation fails because `docs/quality-grades.md` is absent while freshness checks still run (`doc-gardening` failure observed via `scripts/run-script-lint-fixture-tests.sh`).
  - Agent impact: Fresh worktrees can fail validation for environment/setup state rather than code quality, causing noisy failures and slower autonomous convergence.
  - Fix: Add a bootstrap step in validation (generate quality grades if missing) or make freshness checks gracefully skip with actionable guidance when the base artifact is absent.

- **MissingDocumentation | P2** Workflow doc references stale flag name
  - What: `docs/conventions/workflow.md:20` says `--target N`, but active CLI contract uses `--quit-after <N>` (`tools/gardener/src/main.rs`).
  - Agent impact: Agents following docs may invoke invalid flags, creating avoidable command failures and recovery churn.
  - Fix: Replace `--target` with `--quit-after` and extend command-drift linting to catch standalone flag mentions in prose bullets, not only command snippets.