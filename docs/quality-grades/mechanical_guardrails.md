## Mechanical Guardrails Assessment

### Repo-Wide Score: 84
Guardrails are strong and actively enforced: CI runs a full validation pipeline, pre-commit runs formatting plus validation, Clippy is strict (workspace-level denies), and coverage is gated at 90% line coverage via `scripts/test-gardener-coverage.sh`. The main gaps are missing security scanning and a lack of formatter/lint enforcement for shell scripts in CI.

### Per-Domain Scores
- runtime-orchestration: 90 - `tools/gardener/` is protected by CI (`.github/workflows/ci.yml`, `.github/workflows/gardener-coverage.yml`), strict Clippy rules (`Cargo.toml`), coverage gating (`scripts/test-gardener-coverage.sh`), and substantial tests.
- developer-validation-tooling: 78 - `scripts/` has meaningful custom checks (migration wiring, binary blob guard, fixture-tested script behavior), but lacks standard shell linters/formatters and security-oriented automation.

### Key Findings
- CI is mandatory and execution-based: `./scripts/run-validate.sh` plus dedicated coverage workflow both run in GitHub Actions.
- Local pre-commit is real and blocking (`.githooks/pre-commit`), running `rustfmt` on staged Rust files and the full validation script.
- Custom guardrails are unusually strong for repo hygiene (`check-migrations-wired.sh`, `check-binary-blobs.sh`, `run-script-lint-fixture-tests.sh`) and are included in validation.

### Deficiencies
- **MissingTooling | P1** No automated dependency/security scanning in CI
  - What: `.github/workflows/ci.yml` and `.github/workflows/gardener-coverage.yml` do not run `cargo audit`, `cargo deny`, CodeQL, or secret scanning jobs.
  - Agent impact: Autonomous changes can pass functional checks while introducing vulnerable or policy-disallowed dependencies, causing late-stage security rework.
  - Fix: Add a security workflow/job (for example `cargo audit` + `cargo deny` on PRs and scheduled runs, optionally CodeQL) and make it required.

- **FeedbackLoopGap | P1** Rust formatting is enforced only via local hook, not CI
  - What: `.githooks/pre-commit` runs `rustfmt`, but CI validation (`scripts/run-validate.sh`) does not run `cargo fmt --check`.
  - Agent impact: If hooks are not installed or bypassed, formatting drift can land and generate noisy follow-up diffs, wasting agent turns.
  - Fix: Add `cargo fmt --all --check` to `scripts/run-validate.sh` (or directly in `.github/workflows/ci.yml`) so style is server-enforced.

- **MissingTooling | P2** Shell scripts are not linted/formatted with standard tools
  - What: `scripts/*.sh` are tested functionally, but there is no `shellcheck`/`shfmt` step in pre-commit or CI.
  - Agent impact: Agents can introduce quoting/portability issues that pass fixture tests but fail in different environments, increasing flaky runs.
  - Fix: Add `shellcheck scripts/*.sh` and `shfmt -d` (or equivalent) to `scripts/run-validate.sh` and install in CI.