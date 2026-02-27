## Phase 10: Coverage Gates (100% Line Coverage)
Context: [Vision](./00-gardener-vision.md) | [Shared Foundation](./01-shared-foundation.md)
### Changes Required
- Add dedicated coverage command:
  - `npm run test:gardener:coverage` -> `cargo llvm-cov -p gardener --bin brad-gardener ...`.
  - `cargo llvm-cov` is the only authoritative coverage engine for this plan (do not mix with tarpaulin for gates).
- Add enforcement checker:
  - implement as CI/local tooling check (not runtime orchestrator module), e.g. `tools/dev-cli/src/bin/gardener_coverage_gate.rs` or CI parser.
  - fail if line coverage <100 for `tools/gardener/src/**`.
- Wire to validation/CI:
  - include in CI workflow and local validation path for orchestrator changes.
- Add branch coverage assertions for high-risk modules:
  - lease/claim paths
  - quality-domain mapping drift detection
  - quality-grade scoring adjustments
  - quality-grade refresh failure and fallback branches
  - prompt packet construction + manifest hashing branches
  - learning-loop confidence update and decay branches
  - FSM transitions
  - protocol translation and version rejection
  - projection reconciliation/repair logic
  - completion observer-chain idempotency
  - done-means-gone teardown branches
  - startup recovery branches
  - merge failure escalation paths.

### Success Criteria
- Coverage report shows 100.00% lines for orchestrator modules.
- CI fails on any regression below 100.
- Critical failure branches are explicitly tested (not only happy-path lines).

### Phase Validation Gate (Mandatory)
- Run: `cargo test -p gardener --all-targets`
- Run: `npm run test:gardener:coverage` (must report 100.00% lines for `tools/gardener/src/**`).
- Run full E2E binary smoke: `scripts/brad-gardener --parallelism 3 --target 3 --config tools/gardener/tests/fixtures/configs/phase10-full.toml` and verify expected task completion + clean shutdown.
- Run repository gate: `npm run validate`.

### Autonomous Completion Rule
- Continue directly only after all success criteria and this phase validation gate pass.
- Do not wait for manual approval checkpoints.
