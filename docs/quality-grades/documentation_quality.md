## Documentation Quality Assessment

### Repo-Wide Score: 81
The repository is well documented for operators and agents at the workflow/runbook level, with a strong root `README.md`, docs indexing, and anti-drift doc linting/tests. The main gap is API-level Rust documentation in core runtime modules and missing rustdoc-oriented enforcement (`missing_docs`/`cargo doc`) in validation.

### Per-Domain Scores
- runtime-orchestration: 85 - `README.md`, `docs/conventions/workflow.md`, runbooks, and repository map provide clear runtime behavior and operational guidance, but core public modules in `tools/gardener/src` are sparsely rustdoc’d.
- runtime-validation: 74 - Validation behavior is documented and enforced by tests (`agent_doc_integrity_linter`, `command_drift_linter`, `validation_pipeline_docs`), but `tools/gardener/tests` lacks a dedicated navigation/taxonomy README for fast test targeting.
- migration-wiring-fixtures: 54 - Fixture coverage exists (`scripts/fixtures/check-migrations-wired/{passing,missing-migration}`), but fixture intent and expected failure signatures are undocumented.

### Key Findings
- Operator-facing documentation quality is high: strong onboarding, workflow, and troubleshooting content.
- Documentation maintenance has strong mechanical guardrails (`scripts/doc-gardening.sh` plus multiple doc integrity/drift tests).
- Code-facing documentation is uneven: many public Rust APIs in `tools/gardener/src` have little module/API rustdoc and no enforced docs policy.

### Deficiencies

- **[MissingDocumentation | P1] Sparse rustdoc on core public runtime APIs**
  - What: Files such as `tools/gardener/src/lib.rs`, `tools/gardener/src/config.rs`, `tools/gardener/src/startup.rs`, and `tools/gardener/src/worker_pool.rs` expose many `pub` items with minimal `//!` module docs and limited `///` contract docs.
  - Agent impact: Agents must infer invariants from implementation details, increasing exploration turns and raising regression risk in FSM/startup/config edits.
  - Fix: Add `//!` module overviews and targeted `///` docs for high-centrality public types/functions (config resolution, startup audit flow, worker pool lifecycle, protocol mapping).

- **[MissingTooling | P1] No rustdoc-generation or `missing_docs` gate in validation**
  - What: `scripts/run-validate.sh`, workspace `Cargo.toml`, and `tools/gardener/Cargo.toml` do not enforce `cargo doc --no-deps` and do not enable `#![warn/deny(missing_docs)]`.
  - Agent impact: Documentation regressions can pass pre-commit/CI, reducing long-term API discoverability and increasing autonomous planning errors.
  - Fix: Add a docs stage (`cargo doc --no-deps`) to validation and introduce staged `missing_docs` enforcement (warn first, then deny for selected modules).

- **[CoverageGap | P1] Missing test-suite navigation doc for runtime-validation**
  - What: `tools/gardener/tests/` contains many phase/contract/linter tests and fixtures but no `tools/gardener/tests/README.md` mapping scenarios to files and minimal command matrix.
  - Agent impact: Agents run overly broad tests, miss focused suites, and spend extra turns locating relevant fixtures/contracts.
  - Fix: Add `tools/gardener/tests/README.md` with taxonomy (phase, linter, integration), fixture map, and targeted command examples.

- **[MissingDocumentation | P2] Fixture intent is implicit in migration-wiring fixtures**
  - What: `scripts/fixtures/check-migrations-wired/` has `passing` and `missing-migration` fixtures without local documentation explaining expected checker behavior and failure text.
  - Agent impact: Fixture updates become slower and riskier because intent must be reverse-engineered from scripts/tests.
  - Fix: Add a fixture README documenting each case, expected pass/fail outcome, and extension rules for new migration scenarios.