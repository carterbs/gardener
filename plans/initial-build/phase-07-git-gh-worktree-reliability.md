## Phase 7: Git/GH/Worktree Reliability Layer
Context: [Vision](./00-gardener-vision.md) | [Shared Foundation](./01-shared-foundation.md)
### Changes Required
- Add strict command abstractions:
  - `tools/gardener/src/git.rs`
  - `tools/gardener/src/gh.rs`
  - `tools/gardener/src/worktree.rs`.
- Scope this layer to deterministic infrastructure reliability:
  - invariant checks, reconciliation, bounded cleanup, and escalation
  - not full replacement of agent-driven git workflows.
- Replace silent swallow patterns with typed error taxonomy.
- Add recovery policies for:
  - stale lock files
  - detached worktrees
  - already-merged branches
  - merge conflict loops
  - push/rebase failures.
- Implement worktree lifecycle algorithms:
  - create or resume existing meaningful worktree
  - remove/recreate stale empty worktree
  - cleanup on completion
  - prune orphan worktrees at startup.
- Merge behavior:
  - default merge-to-main path
  - configurable merge mode (mergeable-only vs merge-to-main)
  - mandatory merged-state verification before task completion.
  - mandatory post-merge validation command success before task completion.

### Success Criteria
- Hanging worktree scenarios are auto-remediated or deterministically escalated to `P0` tasks.
- Unmerged PR + backlog collision always upgrades to `P0`.
- Merge path is reproducible and test-covered.
- Merge verification contract (`gh pr view` merged state + ancestor check + post-merge validation command) is fully test-covered.
- Worktree create/cleanup operations are idempotent under retries.

### Phase Validation Gate (Mandatory)
- Run: `cargo test -p gardener --all-targets`
- Run: `cargo llvm-cov -p gardener --all-targets --summary-only` (must report 100.00% lines for current `tools/gardener/src/**` code at this phase).
- Run E2E binary smoke: `scripts/brad-gardener --target 1 --config tools/gardener/tests/fixtures/configs/phase07-git-gh-recovery.toml`.

### Autonomous Completion Rule
- Continue directly to the next phase only after all success criteria and this phase validation gate pass.
- Do not wait for manual approval checkpoints.
