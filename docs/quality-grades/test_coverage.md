## Test Coverage Assessment

### Repo-Wide Score: 89
Coverage is strong: `tools/gardener/src` shows broad unit-test presence across most modules, and `tools/gardener/tests` includes 30 integration tests plus 1 e2e test hitting phase flows and TUI/hotkey behavior. The main limiter is relatively thin true end-to-end/system-path coverage compared with the size and criticality of runtime orchestration.

### Per-Domain Scores
- runtime-orchestration: 88 - High module-level coverage and many phase-focused integration tests, but only one explicit e2e path for a complex orchestrator/runtime stack.
- runtime-validation: 93 - Integration suite is extensive and intentionally covers contracts, edge cases, linters, and phase behavior; this is the strongest domain.
- migration-wiring-fixtures: 74 - Fixture-based validation exists, but the fixture surface appears narrow (small pass/fail set), leaving migration wiring edge cases under-exercised.

### Key Findings
- Test distribution is healthy overall, with unusually broad per-module unit coverage in `tools/gardener/src`.
- Integration testing is a clear strength, especially around phase execution and adapter/hotkey/triage edge paths.
- e2e depth is the largest structural gap for confidence in autonomous multi-phase runtime behavior.

### Deficiencies

- **CoverageGap | P1** Limited e2e breadth beyond hotkeys
  - What: Only one e2e file (`tools/gardener/tests/hotkey_pty_e2e.rs`) is present for a runtime that spans startup, orchestration, worker execution, git/worktree flows, and quality grading.
  - Agent impact: Agents can pass unit/integration checks but still fail in real runs due to cross-phase state or process-boundary issues, causing failed autonomous runs and costly reruns.
  - Fix: Add e2e scenarios for full runtime lifecycle (`--config` run, phase progression, worker completion/failure recovery, postmerge/quality outputs) using stable fixtures and deterministic log assertions.

- **MissingTooling | P1** No explicit coverage gate/threshold enforcement visible
  - What: The suite is large, but there is no stated coverage threshold or CI gate tying changed runtime code to minimum line/branch coverage.
  - Agent impact: Regressions can slip in when agents modify low-visibility modules; test count stays high while effective coverage drifts down over time.
  - Fix: Introduce Rust coverage collection in CI (e.g., `cargo llvm-cov`) with per-package thresholds and changed-file coverage checks for `tools/gardener/src/**`.

- **FeedbackLoopGap | P2** Fixture scope for migration wiring is minimal
  - What: `scripts/fixtures/check-migrations-wired/` has only a small fixture set, which likely underrepresents ordering, missing-file, and partial-migration edge cases.
  - Agent impact: Agents may produce migration changes that pass current checks but fail in less common repository states, increasing review churn and runtime break risk.
  - Fix: Expand fixture matrix (out-of-order migrations, duplicate versions, missing baseline, mixed valid/invalid trees) and assert diagnostics in a dedicated integration test table.