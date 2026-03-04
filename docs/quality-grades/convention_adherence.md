## Convention Adherence Assessment

### Repo-Wide Score: 86
Conventions are clearly defined and mostly followed: Rust-first runtime, strict workspace Clippy settings, pre-commit wiring, and substantial contract/lint tests. Adherence is strong across core Rust code and validation scripts, but there are a few policy/tooling inconsistencies that reduce full confidence.

### Per-Domain Scores
- runtime-orchestration: 88 - Consistent Rust module structure, naming, and error handling patterns; strong instrumentation and state-model conventions are enforced by dedicated tests.
- integration-and-contract-testing: 84 - Extensive contract tests codify many conventions (lint config, docs links, validation flow), but some tests are ignored or highly repetitive, which weakens maintenance efficiency.
- developer-automation-and-fixtures: 80 - Validation pipeline and fixture-based script tests are strong, but shell static linting and policy enforcement breadth are incomplete.

### Key Findings
- Workspace-level Clippy policy is explicit and comprehensive (`[workspace.lints.clippy]`), and the crate opts into it with `[lints] workspace = true`.
- Convention enforcement is unusually strong via tests that verify docs, hook behavior, lint config, and instrumentation coverage thresholds.
- Some conventions are documented but not fully enforced mechanically (notably worktree policy), and one lint policy is internally inconsistent in the validation script path.

### Deficiencies

- **ConventionViolation | P1** Clippy policy conflict for `expect_used`
  - What: `Cargo.toml` sets `clippy::expect_used = "deny"`, but `scripts/check-no-warnings.sh` runs Clippy with `-A clippy::expect_used`, partially overriding stated policy.
  - Agent impact: Agents can pass local validation while violating an advertised hard convention, increasing rework and branch-to-branch inconsistency.
  - Fix: Remove `-A clippy::expect_used` from `scripts/check-no-warnings.sh` (or explicitly document/scoped-allow only test targets if intentional).

- **MissingTooling | P1** Worktree policy is mostly documentation-level
  - What: `AGENTS.md` requires git worktrees, but there is no explicit automated guard that fails validation when work is done directly in the root working copy.
  - Agent impact: Autonomous runs can drift into prohibited execution context, causing conflicting local state and harder-to-reproduce failures.
  - Fix: Add a validation check in `scripts/run-validate.sh` (or pre-commit) that detects and rejects root-working-copy execution for agent workflows.

- **MissingTooling | P2** No shell static analysis stage
  - What: Bash scripts are tested with fixtures, but there is no `shellcheck`/`shfmt` gate in the validation pipeline.
  - Agent impact: Script-level portability and quoting issues are caught late (or missed), making automation failures more likely during autonomous execution.
  - Fix: Add `shellcheck` (and optionally `shfmt --check`) to `scripts/run-validate.sh` plus minimal fixture updates for deterministic CI behavior.