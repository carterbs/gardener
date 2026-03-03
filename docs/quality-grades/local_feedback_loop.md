## Local Feedback Loop Assessment

### Repo-Wide Score: 78
Local validation is reproducible and well-wired (`.githooks/pre-commit` -> `scripts/run-validate.sh` -> CI-equivalent checks), with strong command documentation in `README.md` and `docs/conventions/workflow.md`. The loop is slowed by a single heavy “full gate” path (clippy + custom linters + coverage) and no first-class quick/watch workflow for narrow edits.

### Per-Domain Scores
- runtime-orchestration: 76 - Strong Rust test surface (`tools/gardener/tests/*.rs`) and clear run commands, but iterative validation is expensive because canonical flow routes through full coverage gating.
- developer-validation-tooling: 82 - Scripted guardrails are comprehensive and CI-parity is high, but there is no lightweight tier/task-runner UX and no validation-focused watch mode.

### Key Findings
- CI checks are locally reproducible with the same command path (`./scripts/run-validate.sh`) used by both pre-commit and GitHub Actions.
- Developer workflow documentation is explicit and actionable for full validation, including hook setup and remediation.
- Fast inner-loop ergonomics are missing: no `Makefile`/`justfile`/root `package.json` scripts, no quick-mode validator, and no watch-mode for tests/lints.

### Deficiencies

- **[MissingTooling | P1] No first-class quick task runner**
  What: The repo has no `Makefile`/`justfile`/root `package.json` script targets; developers must remember long raw commands (`scripts/run-validate.sh`, `cargo test -p gardener --all-targets`, coverage variants).
  Agent impact: Agents spend extra turns reconstructing commands and run fewer incremental checks, increasing regression escape risk between edits.
  Fix: Add a `justfile` (or `Makefile`) with canonical targets like `quick`, `test`, `test-one`, `lint`, `validate`, `coverage`.

- **[FeedbackLoopGap | P1] Validation path is effectively full-gate only**
  What: `scripts/run-validate.sh` always runs all custom linters plus coverage gate via `scripts/test-gardener-coverage.sh`; no documented/implemented `quick` mode for scoped edits.
  Agent impact: High per-iteration cost discourages frequent local verification, causing larger risky change batches and slower autonomous convergence.
  Fix: Introduce tiered modes (for example `scripts/run-validate.sh --quick|--full`) and document when each mode is acceptable; keep pre-commit/CI on `--full`.

- **[MissingDocumentation | P2] Fast-loop playbook is not explicit**
  What: Docs strongly cover full validation but do not provide a concise “edit-type -> fastest safe command” matrix (e.g., Rust-only unit change vs script-only change).
  Agent impact: Agents default to either over-testing (slow) or under-testing (missed regressions), depending on interpretation.
  Fix: Add a short “Local Feedback Loop” section to `README.md` with command tiers and decision rules.

- **[ConventionViolation | P2] Preflight requires extra tooling for all validations**
  What: `scripts/run-validate.sh --preflight` hard-requires tools like `gh` even when a change may only need Rust/script checks.
  Agent impact: Agents can be blocked from running local validation in minimally provisioned environments, increasing fallback-to-CI behavior.
  Fix: Split preflight into required vs optional tools by stage, or gate `gh` only for checks that actually need it.