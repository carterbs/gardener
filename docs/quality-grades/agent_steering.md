## Agent Steering Assessment

### Repo-Wide Score: 44
The steering is concise and highly specific where it speaks (runtime entry commands, commit/worktree policy), but coverage is narrow. It lacks architecture pointers, explicit test/build workflows beyond runtime launch, and domain-level guidance for tests and scripts, which limits autonomous execution quality.

### Per-Domain Scores
- runtime-orchestration: 70 - Clear Rust-first directive and concrete Gardener runtime invocations provide strong operational guidance for core execution paths.
- integration-and-contract-testing: 28 - No explicit commands or selection strategy for running `tools/gardener/tests/`, so agents must infer test workflows and risk incomplete validation.
- developer-automation-and-fixtures: 24 - `scripts/` usage, fixture expectations, and automation entrypoints are undocumented, leaving agents without actionable script-level instructions.

### Key Findings
- `AGENTS.md` has good signal-to-noise and concrete command specificity, with minimal fluff.
- Steering lacks architecture pointers (module boundaries/where to implement changes), reducing navigation efficiency in `tools/gardener/src/`.
- Testing/build guidance is materially incomplete: runtime invocation is documented, but verification commands and domain-specific test expectations are missing.

### Deficiencies
- **[CoverageGap | P1] Missing domain-level steering beyond runtime launch**
  - What: `AGENTS.md` documents runtime entrypoints but does not cover `tools/gardener/tests/` or `scripts/fixtures/` workflows.
  - Agent impact: Agents can run the app but frequently skip or under-run verification, increasing regression risk and rework.
  - Fix: Add short per-domain sections with exact commands for integration/contract tests and script fixture checks.

- **[MissingTooling | P1] No explicit test/build command matrix**
  - What: There is no canonical list for build, unit, integration, and targeted test runs (workspace vs package-level).
  - Agent impact: Agents spend extra turns discovering commands, may choose slower/full runs unnecessarily, or miss required checks.
  - Fix: Add a compact “Validation Commands” block (e.g., `cargo test -p gardener`, targeted test filters, lint/format/pre-commit command).

- **[MissingDocumentation | P2] No architecture map for `tools/gardener/src/`**
  - What: Steering does not identify key modules (runtime orchestration, worker lifecycle, adapters, phases) or where related changes belong.
  - Agent impact: Higher chance of edits in wrong layer, slower onboarding, and inconsistent fixes across similar issues.
  - Fix: Add a 6-10 line architecture pointer section naming major directories/files and ownership boundaries, with links to deeper docs if available.

- **[FeedbackLoopGap | P2] Pre-commit policy present but failure-handling workflow is incomplete**
  - What: Policy says “do not bypass hooks” but does not specify how to run hooks locally before commit or triage common failures.
  - Agent impact: Agents hit hook failures late in the cycle, causing repeated failed commit attempts and wasted turns.
  - Fix: Add explicit pre-flight command(s) and a short remediation sequence (run hooks, fix, re-run, then commit).