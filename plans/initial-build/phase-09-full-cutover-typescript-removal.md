## Phase 9: Full Cutover + TypeScript Removal
Context: [Vision](./00-gardener-vision.md) | [Shared Foundation](./01-shared-foundation.md)
### Changes Required
- Repoint npm scripts:
  - `package.json` `gardener:run` and `gardener:sync` to Rust equivalents.
- Remove TypeScript orchestrator runtime:
  - `scripts/ralph/*.ts`
  - `scripts/ralph/*.test.ts`.
- Keep task markdown artifacts only as optional generated snapshots (or remove if superseded by new views).
- Update docs:
  - `AGENTS.md`
  - `docs/conventions/workflow.md`
  - add `docs/guides/gardener-orchestrator.md`.
- Cut over quality-grade ownership:
  - replace legacy repo quality-grade refresh entrypoint to delegate to Gardener path (thin wrapper allowed).
  - remove standalone orchestration logic for grade creation outside Gardener runtime.
- Lock runtime behavior for termination modes:
  - single-task mode
  - target-count mode
  - prune-only mode
  - backlog-only mode.
- Define `gardener:sync` contract:
  - runs reconciliation-only flow (startup audits + PR/worktree/backlog synchronization + snapshot export),
  - no worker pool launch,
  - deterministic exit code (`0` healthy sync, non-zero typed sync failure).

### Success Criteria
- No runtime dependency on `tsx scripts/ralph/index.ts` remains.
- `npm run gardener:run` uses Rust path only.
- All documented termination modes work as specified.
- Legacy runtime files are removed from active execution paths.
- Quality-grade document generation is owned by Gardener runtime with no standalone orchestrator-of-record.
- `gardener:sync` behavior is documented and test-covered.

### Phase Validation Gate (Mandatory)
- Run: `cargo test -p gardener --all-targets`
- Run: `cargo llvm-cov -p gardener --all-targets --summary-only` (must report 100.00% lines for current `tools/gardener/src/**` code at this phase).
- Run E2E binary smoke:
  - `npm run gardener:run -- --target 1 --config tools/gardener/tests/fixtures/configs/phase09-cutover.toml`
  - `npm run gardener:sync`
  - verify no runtime invocation of `tsx scripts/ralph/index.ts` remains in execution path.

### Autonomous Completion Rule
- Continue directly to the next phase only after all success criteria and this phase validation gate pass.
- Do not wait for manual approval checkpoints.
