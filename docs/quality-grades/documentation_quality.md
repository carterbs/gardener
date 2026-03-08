## Documentation Quality Assessment

### Repo-Wide Score: 84
The repo has strong operator-facing documentation: a comprehensive root `README.md`, a usable docs index (`docs/README.md`), runbooks, and a repository map. Documentation drift is also guarded by executable checks (`scripts/doc-gardening.sh`, `tools/gardener/tests/docs_integration.rs`, CI running `./scripts/run-validate.sh`). The main gap is API-level Rust contract docs and missing rustdoc/missing-docs enforcement.

### Per-Domain Scores
- runtime-orchestration: 82 - Runtime behavior and workflows are well documented in `README.md` and runbooks, but core public Rust surfaces in `tools/gardener/src/config.rs`, `startup.rs`, and `worker_pool.rs` have limited module/API rustdoc.
- integration-and-contract-testing: 79 - Test coverage and intent are encoded in many descriptive test files, but there is no `tools/gardener/tests/README.md` to map suite taxonomy, fixture purpose, and fastest command paths.
- developer-automation-and-fixtures: 81 - Script-level behavior is reasonably self-describing (`scripts/run-validate.sh`, `doc-gardening.sh`), but fixture semantics (notably `scripts/fixtures/check-migrations-wired/`) are implicit and undocumented.

### Key Findings
- Documentation discoverability is strong at the repo/workflow level (README, docs index, runbooks, repository map).
- Documentation quality has real anti-drift automation via doc-focused tests and validation hooks.
- API discoverability lags workflow discoverability due to sparse rustdoc on central runtime modules and no rustdoc gate in CI.

### Deficiencies

- **[MissingDocumentation | P1] Sparse rustdoc on core public runtime APIs**
  - What: High-centrality modules (`tools/gardener/src/config.rs`, `tools/gardener/src/startup.rs`, `tools/gardener/src/worker_pool.rs`, `tools/gardener/src/lib.rs`) expose many `pub` items with little `//!`/`///` contract documentation.
  - Agent impact: Agents must infer contracts from implementation details, increasing exploration turns and raising risk of incorrect edits in startup/config/FSM paths.
  - Fix: Add module-level `//!` docs and targeted `///` docs for public config resolution, startup lifecycle/report freshness, and worker-pool control flow/failure behavior.

- **[MissingTooling | P1] No rustdoc or missing-docs enforcement in validation**
  - What: CI (`.github/workflows/ci.yml`) and `scripts/run-validate.sh` run validation/lints but do not run `cargo doc --no-deps`, and the crate does not enforce a staged `missing_docs` policy.
  - Agent impact: Documentation regressions can merge silently, degrading API discoverability and causing more planning/implementation errors over time.
  - Fix: Add a validation stage for `cargo doc --no-deps` and introduce staged `missing_docs` enforcement (warn first on core modules, then tighten).

- **[MissingDocumentation | P2] No test-suite navigation README**
  - What: `tools/gardener/tests/` has broad phase/linter/contract tests but lacks a local README explaining suite categories, fixture locations, and targeted command selection.
  - Agent impact: Agents run overly broad test sets and spend extra turns locating the right fixture/suite for a specific change.
  - Fix: Add `tools/gardener/tests/README.md` with test taxonomy, fixture map, and “if you changed X, run Y” command guidance.

- **[CoverageGap | P2] Fixture intent is implicit in migration-wiring script fixtures**
  - What: `scripts/fixtures/check-migrations-wired/{passing,missing-migration}` has no adjacent fixture README describing expected pass/fail semantics and canonical failure signatures.
  - Agent impact: Fixture updates become reverse-engineering work, increasing false fixes and maintenance drift in script validation behavior.
  - Fix: Add `scripts/fixtures/check-migrations-wired/README.md` documenting each fixture case, expected checker output, and extension rules.
