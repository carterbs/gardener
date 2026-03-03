## Agent Steering Assessment

### Repo-Wide Score: 56
The steering docs are concise and high-signal, with concrete runtime entrypoint commands and a useful cross-tool pointer from `CLAUDE.md` to `AGENTS.md`. However, they are under-scoped: they lack architecture mapping, explicit test/verification commands, and domain-specific guidance for `scripts/`, which limits autonomous execution quality.

### Per-Domain Scores
- runtime-orchestration: 64 - `AGENTS.md` gives specific Rust runtime invocation commands and key workflow constraints (worktree + commit policy), but omits module boundaries, test commands, and quality-grading/reconciliation navigation pointers.
- developer-validation-tooling: 34 - No steering content explains how to run or validate `scripts/fixtures/check-migrations-wired` or related maintenance checks, so agents must discover tooling by trial-and-error.

### Key Findings
- Strong specificity where present: concrete `cargo run -p gardener --bin gardener -- ...` commands are directly actionable.
- Excellent signal-to-noise ratio: 21 total lines across both files, no boilerplate bloat.
- Major coverage gap: no architecture pointers or explicit test/build/check commands for either domain.

### Deficiencies

- **MissingTooling | P1** Missing explicit test/verification command matrix
  - What: `AGENTS.md` includes runtime run commands but no exact commands for unit tests, integration tests, linting, formatting, or migration-wiring checks (e.g., `cargo test -p gardener`, targeted test suites under `tools/gardener/tests`, or script validation entrypoints).
  - Agent impact: agents cannot reliably choose the fastest valid verification loop, causing extra discovery turns, skipped checks, or incomplete regression detection.
  - Fix: add a “Verification Commands” section with exact copy/paste commands for build, test (unit/integration), lint/format, and script/fixture checks.

- **CoverageGap | P1** No domain-level guidance for `scripts/` validation tooling
  - What: steering docs do not document purpose, entrypoints, or expected usage for `scripts/fixtures/check-migrations-wired/` and related guardrail automation.
  - Agent impact: maintenance/validation tasks in `developer-validation-tooling` become guesswork, increasing risk of missed migration wiring regressions and failed CI parity.
  - Fix: add a short `scripts/` subsection in `AGENTS.md` (or `scripts/AGENTS.md`) listing command entrypoints, expected inputs/outputs, and when to run them.

- **MissingDocumentation | P2** Architecture pointers are absent
  - What: no map of key modules in `tools/gardener/src` (orchestration phases, adapters, grading logic, CLI boundaries) and where tests live.
  - Agent impact: slower onboarding and more navigation errors; agents spend turns searching instead of making correct localized edits.
  - Fix: add a compact “Architecture Pointers” block naming 5-10 high-value paths (runtime core, grading, adapters, CLI, integration tests) with one-line purpose each.

- **ConventionViolation | P2** Tool-specific duplication risks divergence
  - What: `CLAUDE.md` only says “read AGENTS.md”; this is good for progressive disclosure, but no explicit statement that `AGENTS.md` is canonical for all agents (Codex/Cursor/etc.) and no sync policy.
  - Agent impact: future tool-specific files may drift, causing inconsistent behavior across agents and conflicting instructions.
  - Fix: add one line in both files declaring `AGENTS.md` as canonical cross-tool steering and requiring any tool-specific file to remain a thin pointer.