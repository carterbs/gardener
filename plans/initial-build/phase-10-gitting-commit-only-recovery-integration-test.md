## Phase 10: Integration test for commit-only gitting pre-commit recovery
### Scope
- Objective: add a regression test that proves commit-only gitting retries or fails only according to recovery contract when pre-commit hooks alter the index or working tree.
- Target code paths: `worker.rs` gitting recovery branch and `prompt_registry.rs` commit-only gitting prompt contract.

### Problem statement
- Recent failures show `gitting` returns terminal success while repository remains dirty (`worker.gitting.dirty_worktree`), usually due to hook-applied formatting changes after commit.
- Existing check currently had to be corrected to allow a controlled recovery attempt in commit-only mode.
- A stronger integration test is needed so this sequence is not regressed.

### Implementation plan
1. Add dedicated test fixture workspace in `tools/gardener/tests/fixtures/repos/gitting-commit-recovery/`.
   1. Minimal Rust crate with a couple of tracked files.
   2. `.pre-commit` hook that intentionally transforms a file (e.g., rewrites spacing) and exits non-zero on first run to force agent recovery.
   3. Documented setup helpers for creating, seeding a `.cache` sqlite, and cleaning temp dirs.

2. Add integration test file `tools/gardener/tests/phase10_gitting_recovery_integration.rs`.
   1. Start gardener runtime in test mode that executes the actual `run_with_runtime` path or `execute_task` with a deterministic fake adapter queue.
   2. Configure execution with `git_output_mode = "commit_only"` and a task summary that goes to `gitting`.
   3. Assert:
      - first gitting pass returns terminal success.
      - pre-commit hook effect is intentionally created.
      - second recovery pass runs and either produces clean worktree or logs clean failure reason.
      - final summary includes recovery path reason when second pass cannot fully clean changes.
   4. Capture and assert log events:
      - `worker.gitting.dirty_worktree` (first detection)
      - `worker.gitting.dirty_worktree_recovery_failed` (if recovery does not clear tree)

3. Extend fake command queue assertions (or fixture logs) to ensure required ordering:
   - pre-gitting setup -> plan/doing/reviewing -> gitting attempt -> git status check.
   - recovery attempt uses same commit-only prompt and then final status check.

4. Add a second assertion test for pre-commit recovery success.
   - Hook should fail once and then succeed after agent fix step.
   - Assert worker completes with `final_state == Complete` and no dirty-worktree failure.

### Validation
- `cargo test -p gardener --test phase10_gitting_recovery_integration`
- Keep this test behind the existing runtime/integration test guard conventions to avoid nondeterminism in normal CI runs.
- Update any relevant test fixture docs for hook behavior and expected run logs.

### Definition of done
- Commit-only gitting path emits a recoverable dirty-worktree attempt and still fails only when still dirty after recovery.
- Regression test covers this behavior end-to-end and blocks reverting to immediate failure on first dirty worktree in commit-only mode.
