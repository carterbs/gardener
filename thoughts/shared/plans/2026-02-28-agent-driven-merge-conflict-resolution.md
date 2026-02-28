# Plan: Agent-Driven Merge Conflict Resolution

## Context

Worker-3 failed on task `manual:tui:auto-1772248472000` because its commit (`fc56eb1`) conflicted with `867bfb0` on main — both redesigned the same triage wizard UI in `tui.rs`. The current merge phase in the worker FSM calls `git.rebase_onto_local("main")`, and on conflict immediately aborts the rebase and returns `WorkerState::Failed` with a `failure_reason`. The worker pool then sees that failure reason, sets `shutdown_error`, and calls `request_interrupt()` — **killing the entire application**.

The fix: replace the hard-fail rebase with an agent-driven conflict resolution step. If the agent can't resolve, the task goes to an "unresolved" queue instead of crashing the app.

## Changes (in dependency order)

### 1. `tools/gardener/src/git.rs` — Add non-aborting rebase

Add `RebaseResult` enum and `try_rebase_onto_local()` that returns `Conflict` instead of aborting + erroring. Add `abort_rebase()` helper for explicit cleanup.

```rust
pub enum RebaseResult {
    Clean,
    Conflict { stderr: String },
}

pub fn try_rebase_onto_local(&self, base: &str) -> Result<RebaseResult, GardenerError>
// On conflict: does NOT call `git rebase --abort`, returns Ok(Conflict{...})
// On clean: returns Ok(Clean)
// On unexpected error (git not found, etc.): returns Err

pub fn abort_rebase(&self) -> Result<(), GardenerError>
// Runs `git rebase --abort`
```

Keep existing `rebase_onto_local()` unchanged.

### 2. `tools/gardener/src/prompt_registry.rs` — Conflict resolution prompt

Add `conflict_resolution_template()` following existing template patterns. The prompt instructs the agent to:
- Find conflicted files via `git diff --name-only --diff-filter=U`
- Examine main's history via `git log main --oneline -5 -- <file>` to understand why main changed
- Read its own `[task_packet]` to understand what it was doing
- Decide: **resolve** (edit + `git add` + `git rebase --continue`), **skip** (`git rebase --skip` if work is superseded), or **unresolvable** (`git rebase --abort`)
- Run validation after resolution
- Output JSON: `{ resolution: "resolved"|"skipped"|"unresolvable", reason: "...", merge_sha: "..." }`

Add builder: `pub fn with_conflict_resolution(mut self) -> Self` that inserts the template at `WorkerState::Merging` (same pattern as `with_retry_rebase()` at line 50).

### 3. `tools/gardener/src/backlog_store.rs` — Add `Unresolved` status

- Add `Unresolved` variant to `TaskStatus` enum
- Update `as_str()` → `"unresolved"` and `from_db()` → parse `"unresolved"`
- Add `WriteCmd::MarkUnresolved` variant (copy pattern from `MarkComplete`)
- Add `pub fn mark_unresolved(&self, task_id, lease_owner)` method
- Add private `fn mark_unresolved(conn, task_id, lease_owner, now)` — SQL sets `status = 'unresolved'`, clears lease
- Handle in writer thread match arm
- Exclude `Unresolved` from `recover_stale()` — these should NOT be auto-recovered

### 4. `tools/gardener/migrations/0003_backlog.sql` — Schema migration

The `0001_backlog.sql` has `CHECK(status IN ('ready','leased','in_progress','complete','failed'))`. SQLite can't ALTER CHECK constraints, so rebuild the table:

```sql
CREATE TABLE backlog_tasks_new (...same columns..., CHECK(status IN (..., 'unresolved')));
INSERT INTO backlog_tasks_new SELECT * FROM backlog_tasks;
DROP TABLE backlog_tasks;
ALTER TABLE backlog_tasks_new RENAME TO backlog_tasks;
-- Recreate indexes
```

Register as `(3_i64, include_str!("../migrations/0003_backlog.sql"))` in `run_migrations()` at line 663.

### 5. `tools/gardener/src/worker.rs` — Agent-driven conflict resolution

**Replace lines 512-534** (the `if let Err(err) = git.rebase_onto_local("main")` block) with:

```
match git.try_rebase_onto_local("main") {
    Ok(RebaseResult::Clean) => { /* proceed to normal merge below */ }
    Ok(RebaseResult::Conflict { stderr }) => {
        // Build conflict-resolution registry
        let cr_registry = registry.clone().with_conflict_resolution();
        // Run agent turn with conflict resolution prompt
        let cr_result = run_agent_turn(TurnContext { registry: &cr_registry, ... })?;
        // Parse output
        match resolution {
            Resolved => { /* fall through to normal merge flow */ }
            Skipped  => { return Ok(Complete, failure_reason: None) }
            Unresolvable => {
                git.abort_rebase()?;
                return Ok(Failed, failure_reason: None)  // None = non-fatal
            }
        }
    }
    Err(err) => { /* unexpected error, return Failed with reason (fatal) */ }
}
```

Add parsing function `parse_conflict_resolution_output(payload) -> ConflictResolutionOutput` following the pattern of `parse_merge_output()` at line 1112.

Add `ConflictResolutionOutput` struct with `resolution: String`, `reason: String`, `merge_sha: Option<String>`.

### 6. `tools/gardener/src/worker_pool.rs` — Non-fatal failure path

**Replace lines 302-309** failure handling. Currently:
```rust
if let Some(reason) = &summary.failure_reason {
    shutdown_error = Some(...); request_interrupt();
} else {
    let _ = store.release_lease(...);
}
```

New logic: when `final_state == Failed` AND `failure_reason.is_none()`:
- Call `store.mark_unresolved(task_id, worker_id)` instead of `release_lease`
- Log `"worker.task.unresolved"`
- Do NOT set `shutdown_error` or call `request_interrupt()`
- Worker becomes available for next task

This uses the existing convention: `failure_reason = Some(...)` means fatal, `None` means retryable/non-fatal. The existing `else` branch already does `release_lease` for non-fatal. We refine it: check if it came from merging (Failed + no reason) → mark unresolved instead of releasing for retry.

### 7. `tools/gardener/src/tui.rs` + `worker_pool.rs` — Dashboard updates

- Add `unresolved: usize` field to `QueueStats` (line 46)
- In `dashboard_snapshot()` (worker_pool.rs:597): add `TaskStatus::Unresolved => stats.unresolved += 1`
- Display unresolved count in the TUI queue stats area
- Update all `QueueStats` construction sites (search for `QueueStats {`)

## Files Modified

| File | Change |
|------|--------|
| `tools/gardener/src/git.rs` | Add `RebaseResult`, `try_rebase_onto_local()`, `abort_rebase()` |
| `tools/gardener/src/prompt_registry.rs` | Add `conflict_resolution_template()`, `with_conflict_resolution()` |
| `tools/gardener/src/backlog_store.rs` | Add `Unresolved` status, `mark_unresolved()`, writer match arm |
| `tools/gardener/migrations/0003_backlog.sql` | New migration: rebuild table with updated CHECK constraint |
| `tools/gardener/src/worker.rs` | Replace hard-fail rebase with agent conflict resolution flow |
| `tools/gardener/src/worker_pool.rs` | Non-fatal failure path: mark unresolved instead of shutdown |
| `tools/gardener/src/tui.rs` | Add `unresolved` to `QueueStats` |

## Verification

1. `cargo build` — confirm compilation
2. `cargo test` — all existing tests pass
3. `cargo clippy` — no new warnings
4. Manual test: create a scenario with conflicting worktree branches and verify the agent is invoked for resolution instead of hard-failing
5. Verify unresolved tasks show up in the TUI dashboard and don't trigger application shutdown
