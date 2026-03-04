## Risk Exposure Assessment

### Repo-Wide Score: 68
Risk is elevated by concentration of logic in very large runtime files (`tui.rs`, `backlog_store.rs`, `worker_pool.rs`, `startup.rs`) plus 29/98 untested source files. Debt-marker volume is low (13 total), and many high-complexity files do include unit tests, which keeps risk below the high-risk band. Overall this is a test-rich repo with meaningful blind spots in orchestration boundaries.

### Per-Domain Scores
- runtime-orchestration: 62 - Highest exposure: deepest complexity sits here, and several runtime modules in the untested list (`config.rs`, `phase_cli.rs`, `plan_phase.rs`, `merge_loop.rs`, `pr_audit.rs`, `main.rs`) are operationally sensitive.
- integration-and-contract-testing: 74 - Strong suite breadth (phase/contract/lint/e2e tests) reduces regression risk, but coverage is uneven against all runtime boundary modules.
- developer-automation-and-fixtures: 71 - Scope is smaller, but fixture/script paths include untested files and can silently drift from runtime expectations.

### Key Findings
- Complexity is heavily concentrated in a few orchestration files, creating large regression blast radius per change.
- Test density is good overall, including many inline tests in complex files, but critical untested runtime modules remain.
- Debt markers are not the primary risk driver; coverage gaps around execution boundaries are.

### Deficiencies
- **[CoverageGap | P0] Untested runtime boundary modules**
- What: Multiple runtime entry/boundary files are untested (`tools/gardener/src/config.rs`, `phase_cli.rs`, `plan_phase.rs`, `merge_loop.rs`, `pr_audit.rs`, `main.rs`, plus others from the untested list).
- Agent impact: Autonomous runs can pass local checks while still failing in real orchestration paths (startup, planning, merge, CLI pathing), causing failed runs and wasted remediation turns.
- Fix: Add focused contract/integration tests for each boundary module using existing phase-fixture patterns in `tools/gardener/tests/fixtures/configs`.

- **[FeedbackLoopGap | P1] Monolithic files slow safe iteration**
- What: `tui.rs` (~4k LOC) and other very large files hold many concerns in one unit, increasing coupling and branch interactions.
- Agent impact: Small edits require broad retesting/context loading, increasing token/tool usage and raising accidental regression probability.
- Fix: Split by responsibility (rendering, input handling, state mapping, formatting/parsing) and keep tests co-located per new module.

- **[MissingTooling | P1] No hard guard against untested-source drift**
- What: The repo surfaces untested-file reporting, but current quality gates still allow significant untested source inventory.
- Agent impact: Agents cannot rely on CI to reject newly uncovered critical paths, so risk accumulates invisibly over time.
- Fix: Add CI policy gates (for example: fail on new untested files in runtime-orchestration paths, then ratchet toward stricter thresholds).

- **[MissingDocumentation | P2] Limited executable docs for orchestration invariants**
- What: High-risk flows (worker lifecycle/state transitions/merge loop expectations) are encoded mainly in code/tests, with limited concise invariant docs.
- Agent impact: Agents spend extra turns inferring implicit rules, increasing mis-edits in stateful paths.
- Fix: Add short runbooks describing invariants and failure modes for worker FSM, startup sequencing, and merge-loop decision logic, each linked to owning tests.