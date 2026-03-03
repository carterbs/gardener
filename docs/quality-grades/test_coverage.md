## Test Coverage Assessment

### Repo-Wide Score: 84
Coverage is strong in `tools/gardener`, with broad unit coverage plus many integration phase tests and one e2e hotkey test. The main drag on the score is weaker explicit coverage in `scripts/` validation tooling and limited e2e depth relative to the runtime surface area.

### Per-Domain Scores
- runtime-orchestration: 90 - `tools/gardener` has extensive unit coverage across core modules plus 29 integration tests and 1 e2e test spanning startup, scheduling, adapters, triage, rendering, hotkeys, and CLI paths.
- developer-validation-tooling: 42 - The `scripts/` area appears lightly exercised directly; `scripts/fixtures/check-migrations-wired/` is present, but dedicated script-focused tests are limited/implicit rather than clearly comprehensive.

### Key Findings
- Integration coverage for runtime phases is unusually complete (`phase03` through `phase12`, adapter edges, triage paths, hotkeys, TUI, CLI smoke), which materially boosts confidence.
- Unit tests are distributed across most Rust source modules, suggesting good module-level regression protection.
- e2e coverage is thin (single `hotkey_pty_e2e.rs`), leaving multi-phase runtime behavior and script validation workflows underrepresented end-to-end.

### Deficiencies

- **[CoverageGap | P1] Limited end-to-end runtime scenario coverage**
  - What: Only one e2e file (`tools/gardener/tests/hotkey_pty_e2e.rs`) covers a narrow interaction path; no broad e2e for full orchestration (`startup -> triage -> execution -> render`).
  - Agent impact: Autonomous changes can pass unit/integration tests but still fail in real run sequencing, causing failed runs and extra diagnosis turns.
  - Fix: Add 2-3 deterministic e2e scenarios that execute the Rust entrypoint with fixtures/configs and assert phase transitions, outputs, and failure handling across the full lifecycle.

- **[CoverageGap | P1] Script/validation-tooling domain is under-tested directly**
  - What: `scripts/fixtures/check-migrations-wired/` indicates migration-wiring validation behavior, but `scripts/` itself lacks clearly scoped, direct test suites in the provided inventory.
  - Agent impact: Agents modifying guardrail scripts can introduce regressions in repo validation that are only caught late (or missed), slowing autonomous iteration.
  - Fix: Create explicit integration tests for each script contract (input fixture -> command -> expected exit/status/output), including negative/failure cases.

- **[FeedbackLoopGap | P2] Heavy reliance on file-presence style checks over behavior assertions in quality tooling**
  - What: Many quality modules are unit-tested, but cross-module behavioral assertions for grading outputs (combined evidence, scoring drift, threshold edges) appear limited.
  - Agent impact: Agents may produce subtly wrong quality grades/prompts that look structurally valid, leading to missed regressions and lower trust in automated assessment output.
  - Fix: Add golden/integration tests for the full quality pipeline (`quality_*` modules together) with stable fixtures and expected grade/evidence bundles to detect semantic drift.