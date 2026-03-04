## Test Coverage Assessment

### Repo-Wide Score: 84
Coverage is strong overall: most Rust runtime modules appear to include unit tests, and there is a broad integration suite across orchestration phases. The main limiter is relatively thin true e2e coverage (only one explicit e2e test file), which leaves some cross-component runtime behavior less protected than the integration/unit density suggests.

### Per-Domain Scores
- runtime-orchestration: 85 - Very high unit-test presence across `tools/gardener/src/` plus multiple phase-oriented integration tests covering critical runtime paths.
- integration-and-contract-testing: 88 - Large and diverse integration/contract suite in `tools/gardener/tests/`, including CLI smoke, phase tests, linters, and one e2e path.
- developer-automation-and-fixtures: 72 - Script/fixture coverage exists via targeted integration tests, but this area is small and appears less comprehensively validated than runtime orchestration.

### Key Findings
- Phase-based integration tests are a major strength and improve confidence in orchestration behavior across worker lifecycle and adapter paths.
- Unit tests are widespread in runtime modules, indicating good file-level coverage for core Rust logic.
- End-to-end depth is the primary gap; a single e2e test is unlikely to catch all real multi-worker/runtime regressions.

### Deficiencies
- **[CoverageGap | P1] Limited end-to-end scenario breadth**
  - What: Only one explicit e2e file (`tools/gardener/tests/hotkey_pty_e2e.rs`) is present versus many integration/unit tests.
  - Agent impact: Cross-phase regressions can slip through until late, causing failed autonomous runs and expensive retry/debug cycles.
  - Fix: Add 3-5 additional e2e flows for full runtime execution (`gardener` bin), including multi-worker sync/quit paths and failure-recovery scenarios.

- **[FeedbackLoopGap | P1] Uneven validation of automation scripts/fixtures**
  - What: `scripts/fixtures/check-migrations-wired/` and related script behavior appear to have limited dedicated scenario coverage.
  - Agent impact: Agents relying on automation checks may receive false confidence or noisy failures, increasing wasted turns during triage.
  - Fix: Add table-driven integration tests that execute script workflows against fixture variants (valid, malformed, drifted) with strict expected outputs.

- **[ObservabilityGap | P2] Log/telemetry behavior likely tested narrowly**
  - What: There are telemetry/log tests (`otel_log_query.rs`, `watch_otel_logs_script.rs`), but breadth of malformed/high-volume/ordering cases appears limited from file distribution.
  - Agent impact: Agents can miss root-cause signals or misclassify failures when log formats drift, slowing autonomous diagnosis.
  - Fix: Expand contract tests for OTEL JSONL parsing/streaming with fuzzed malformed events, out-of-order records, and high-volume truncation boundaries.