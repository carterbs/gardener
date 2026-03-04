## Mechanical Guardrails Assessment

### Repo-Wide Score: 84
The repo has strong baseline guardrails: CI is present, pre-commit runs validation, clippy is enforced with denied lints, and a hard 90% line-coverage gate is wired into CI. It also includes custom lint scripts for migration wiring, binary artifact blocking, and doc/command drift checks. Main gaps are missing security scanning and missing explicit formatter enforcement in CI.

### Per-Domain Scores
- runtime-orchestration: 86 - `tools/gardener/src/` is protected by clippy (`-D warnings`), CI validation, and coverage gating via `scripts/test-gardener-coverage.sh`, with additional custom checks in `scripts/run-validate.sh`.
- runtime-validation: 83 - `tools/gardener/tests/` is exercised through coverage/test pipelines and targeted doc/command contract tests, but no dedicated mutation/security/dependency risk checks are enforced.
- migration-wiring-fixtures: 76 - Fixture-backed migration wiring checks are present (`scripts/check-migrations-wired.sh` + fixture tests), but guardrails are narrow in scope and rely on shell-script conventions rather than broader static enforcement.

### Key Findings
- CI and pre-commit both route through `scripts/run-validate.sh`, creating a mostly unified enforcement path.
- Mechanical quality checks are unusually strong for repo-specific risks (migration wiring, binary blob prevention, skills sync, doc drift).
- Security/dependency guardrails (CodeQL, `cargo audit`, `cargo deny`, Dependabot) are not detected.

### Deficiencies

- **MissingTooling | P1** Missing security/dependency scanning
  - What: No workflows or scripts for `cargo audit`, `cargo deny`, CodeQL, Semgrep, or Dependabot config were found (only `.github/workflows/ci.yml` and `.github/workflows/gardener-coverage.yml` exist).
  - Agent impact: Agents can ship vulnerable dependencies or risky code patterns without automated detection, increasing regression and incident risk after “green” CI.
  - Fix: Add a CI security job (at minimum `cargo audit` + `cargo deny check`) and enable Dependabot/CodeQL in `.github/`.

- **FeedbackLoopGap | P1** Formatter compliance is not explicitly CI-gated
  - What: `.githooks/pre-commit` runs `rustfmt` on staged files, but CI/validation does not run `cargo fmt --check`; `scripts/run-validate.sh` only checks formatter availability.
  - Agent impact: If hooks are not installed or bypassed in a local environment, formatting drift can land and force later cleanup churn across autonomous runs.
  - Fix: Add `cargo fmt --all -- --check` to `scripts/run-validate.sh` (or a dedicated CI step) so formatting is enforced server-side.

- **CoverageGap | P2** CI path filters can miss guardrail-relevant changes
  - What: `.github/workflows/ci.yml` only triggers on selected paths (`tools/gardener/**`, `scripts/**`, Cargo files, skills dirs), not on guardrail files like `.githooks/pre-commit` or broader repo config.
  - Agent impact: Changes to enforcement plumbing may merge without re-running validation, causing silent degradation in future agent feedback loops.
  - Fix: Expand workflow `paths` coverage (or add a lightweight always-on validation workflow) to include hook/workflow/config files that affect guardrail behavior.