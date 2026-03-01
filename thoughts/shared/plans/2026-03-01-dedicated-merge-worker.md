# Dedicated Merge Worker

## Overview

Replace the global `MERGE_PHASE_LOCK` mutex with a dedicated merge worker that processes a merge queue. Doing workers hand off completed-and-approved tasks to the merge queue and immediately claim new backlog items. The merge worker processes one PR at a time — naturally serialized, no mutex needed.

## Current State Analysis

### The Bottleneck

- `worker.rs:67-71` — A process-global `OnceLock<Mutex<()>>` serializes all workers through the merge phase
- `worker.rs:621-623` — Workers block on `.lock()`, emitting `MergeLockWaiting` while idle
- `merge_loop.rs:28` — Mergeability polling uses 30s intervals for up to 10 attempts (5 min worst case)
- During this time, blocked workers hold their worktrees, agent sessions, and backlog leases while doing zero work

### Threading Model

- `worker_pool.rs:190` — `std::thread::scope` manages all worker threads
- `worker_pool.rs:183-186` — A single `mpsc::channel()` carries `WorkerResultMessage` tuples `(slot_idx, task_id, Result<WorkerRunSummary>)`
- `worker_pool.rs:230-461` — Main loop receives results, processes them, and immediately re-claims for the same slot
- Workers are spawned per-slot, not per-task — a slot is reused across multiple tasks

### The Natural Seam

The split point is `worker.rs:603-645`, between the review verdict and `run_merge_loop`. At this point, all merge-required state is materialized:

| Variable | Type | Source |
|---|---|---|
| `worktree_path` | `PathBuf` | `worker.rs:204` |
| `branch` | `String` | `worker.rs:205` |
| `pr_number` | `u64` | `worker.rs:498` |
| `identity` | `WorkerIdentity` | `worker.rs:198` |
| `task_id` | `String` | function param |
| `task_summary` | `&str` | function param |
| `attempt_count` | `i64` | function param |
| `cfg` | `&AppConfig` | function param |
| `process_runner` | `&dyn ProcessRunner` | function param |
| `scope` | `&RuntimeScope` | function param |
| `logs` | `Vec<WorkerLogEvent>` | accumulated across prior turns |

The `factory`, `registry`, `learning_loop`, `git`, and `gh` clients are all reconstructible from the above — they're created from `cfg`/`process_runner`/`worktree_path` earlier in the function.

### Backlog Store

- `backlog_store.rs:21-28` — Statuses: `Ready`, `Leased`, `InProgress`, `Complete`, `Failed`, `Unresolved`
- No `merge_pending` state exists today
- `mark_complete` at `backlog_store.rs:966-973` accepts from `leased` or `in_progress`
- Adding a new status requires: new migration (CHECK constraint), enum variant, `as_str`/`from_db`, and updates to `count_active_tasks`/`count_tasks_by_priority`/`recover_stale`

### Pre-existing Issue (out of scope)

`worker.rs:584-586` — The NeedsChanges review path sets FSM state to `Doing` but falls through into the merge phase. This refactor fixes it implicitly: only the `Approve` path sends to the merge queue.

## Desired End State

1. Doing workers run Understand → Plan → Do → Git → PR → Review, then hand off to a merge queue and immediately claim the next backlog item
2. A single merge worker thread processes the queue serially — poll mergeability, handle CI failures, resolve conflicts, merge
3. The `MERGE_PHASE_LOCK` mutex is deleted
4. The TUI shows the merge worker as a distinct row with its own state
5. Backlog tasks in the merge queue have status `merge_pending` so the dashboard and recovery logic understand them
6. Worktree ownership transfers cleanly from the doing worker to the merge worker, with the merge worker handling teardown

## What We're NOT Doing

- Changing the merge loop logic itself (`merge_loop.rs`) — it works, it just runs on a different thread
- Parallel merging — still one at a time, by design
- Changing the `--parallelism` CLI semantics — it still means "number of doing workers"
- Adding a configurable number of merge workers — exactly one, always

## Implementation Approach

The merge queue is an `mpsc` channel carrying `MergeRequest` structs. The merge worker is a thread spawned inside the same `std::thread::scope` as the doing workers, reading from the receiver. Doing workers send to the channel instead of acquiring the mutex. Results flow back through a second channel (or the existing `WorkerResultMessage` channel with a discriminant).

---

## Phase 1: Define `MergeRequest` and the merge queue channel

### Overview
Create the data structure that crosses the boundary between doing workers and the merge worker. This is the handoff contract.

### Changes

**`tools/gardener/src/worker.rs`**

Add a new struct:

```rust
pub struct MergeRequest {
    pub slot_idx: usize,
    pub task_id: String,
    pub task_summary: String,
    pub attempt_count: i64,
    pub worker_id: String,
    pub session_id: String,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub pr_number: u64,
    pub logs: Vec<WorkerLogEvent>,
}
```

This captures everything `MergeLoopContext` needs that can't be reconstructed from `cfg`/`scope`/`process_runner`. The `slot_idx` tells the pool which worker row to update on completion.

**`tools/gardener/src/worker.rs`**

Add a new return variant to distinguish "task done" from "task handed off to merge":

```rust
pub enum WorkerOutcome {
    Completed(WorkerRunSummary),
    HandoffToMerge(MergeRequest),
}
```

### Success Criteria
- `MergeRequest` and `WorkerOutcome` compile
- No behavior change yet

---

## Phase 2: Split `execute_task_live` at the merge seam

### Overview
Refactor `execute_task_live` to return `WorkerOutcome::HandoffToMerge` when the review approves, instead of proceeding to the merge phase inline. Extract the merge-and-teardown tail into a separate function.

### Changes

**`tools/gardener/src/worker.rs` — `execute_task_live`**

At the current seam (`worker.rs:599-601`, the Approve branch):

- Instead of falling through to the merge lock, construct a `MergeRequest` from the in-scope variables and return `Ok(WorkerOutcome::HandoffToMerge(request))`
- The NeedsChanges→Doing fallthrough bug is now impossible: `HandoffToMerge` is only returned from the `Approve` branch

**`tools/gardener/src/worker.rs` — new `execute_merge_phase`**

Extract lines 603-738 (merge lock acquisition through teardown) into:

```rust
pub fn execute_merge_phase(
    req: &MergeRequest,
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
) -> Result<WorkerRunSummary, GardenerError>
```

This function:
1. Reconstructs `WorkerIdentity`, `GitClient`, `GhClient`, `AdapterFactory`, `PromptRegistry`, `LearningLoop` from `req` + `cfg` + `process_runner`
2. Calls `run_merge_loop` (unchanged)
3. Runs post-merge validation
4. Calls `teardown_after_completion`
5. Returns `WorkerRunSummary`

No merge lock acquisition — the caller (merge worker thread) is single-threaded by construction.

### Success Criteria
- `execute_task_live` returns `WorkerOutcome` instead of `WorkerRunSummary`
- `execute_merge_phase` produces the same `WorkerRunSummary` as the old inline merge path
- Existing tests pass (the simulated path returns `WorkerOutcome::Completed` directly)

### Confirmation Gate
Verify with `cargo test` and `cargo clippy` that the split compiles and existing tests pass. The pool doesn't use the new types yet, so behavior is unchanged.

---

## Phase 3: Add `merge_pending` backlog status

### Overview
The backlog store needs to know that a task is in the merge queue — not `in_progress` (that would look like a doing worker owns it), not `complete` (it isn't merged yet). A new `merge_pending` status bridges the gap.

### Changes

**`tools/gardener/migrations/NNNN_merge_pending.sql`** (new migration)

Add `'merge_pending'` to the CHECK constraint. Follow the existing drop-and-recreate pattern from `0003_backlog.sql`.

**`tools/gardener/src/backlog_store.rs`**

- Add `MergePending` variant to `TaskStatus` enum (line ~25)
- Add `"merge_pending"` to `as_str()` and `from_db()` match arms
- Add `mark_merge_pending(&self, task_id: &str, worker_id: &str)` method:
  - `UPDATE SET status = 'merge_pending', lease_owner = NULL WHERE task_id = ? AND status = 'in_progress' AND lease_owner = ?`
  - Clears `lease_owner` since the doing worker is releasing ownership
- Add `claim_merge_pending(&self, merge_worker_id: &str)` method:
  - `UPDATE SET status = 'in_progress', lease_owner = ? WHERE task_id = (SELECT task_id FROM backlog_tasks WHERE status = 'merge_pending' ORDER BY last_updated ASC LIMIT 1) RETURNING *`
  - FIFO ordering — oldest merge-pending task first
- Update `count_active_tasks` (line ~675) to include `merge_pending` as active
- Update `count_tasks_by_priority` (line ~656) to include `merge_pending`
- Update `recover_stale` (line ~1033) to recover `merge_pending` → `ready` (if the merge worker crashes, tasks need to be re-done from scratch since the worktree may be in an inconsistent state)

**`tools/gardener/src/tui.rs`** (dashboard stats)

Update `QueueStats` to track `merge_pending` count. Display in the dashboard.

### Success Criteria
- Migration applies cleanly
- `mark_merge_pending` + `claim_merge_pending` round-trip in unit tests
- `recover_stale` recovers orphaned `merge_pending` tasks to `ready`
- Existing backlog tests pass

---

## Phase 4: Wire the merge worker thread into the worker pool

### Overview
This is the core integration. The worker pool spawns one additional thread — the merge worker — alongside the N doing workers. Doing workers send `MergeRequest`s through a channel. The merge worker processes them one at a time and sends results back through the existing result channel.

### Changes

**`tools/gardener/src/worker_pool.rs`**

1. Create a merge queue channel alongside the result channel:
   ```rust
   let (merge_tx, merge_rx): (mpsc::Sender<MergeRequest>, mpsc::Receiver<MergeRequest>) = mpsc::channel();
   ```

2. Spawn the merge worker thread inside `std::thread::scope`:
   ```rust
   let merge_result_tx = tx.clone(); // reuse the result channel
   scope_guard.spawn(move || {
       loop {
           let req = match merge_rx.recv() {
               Ok(req) => req,
               Err(_) => break, // channel closed, all doing workers done
           };
           let slot_idx = req.slot_idx;
           let task_id = req.task_id.clone();
           // claim from backlog: merge_pending → in_progress
           let _ = store.claim_merge_pending("merge-worker");
           let result = execute_merge_phase(&req, cfg, process_runner, scope);
           let _ = merge_result_tx.send((slot_idx, task_id, result));
       }
   });
   ```

   Note: the merge worker uses a special sentinel slot index (e.g., `usize::MAX`) or a new enum variant for `WorkerResultMessage` so the pool can distinguish merge completions from doing worker completions.

3. Modify the doing worker result handling:

   When a doing worker returns `WorkerOutcome::HandoffToMerge(req)`:
   - Call `store.mark_merge_pending(&task_id, &worker_id)` to transition the backlog task
   - Send `req` through `merge_tx`
   - **Immediately** claim the next backlog task for this slot and spawn a new doing worker thread (existing re-claim logic at lines 378-458)
   - Update the worker's TUI row to show the new task, not "merging"

   When `WorkerOutcome::Completed(summary)` arrives (only from simulated mode now):
   - Handle as before

4. When a merge result arrives on `rx`:
   - Call `store.mark_complete` or `store.mark_unresolved` as appropriate
   - Increment `completed` counter
   - Update TUI (the merge worker row, not a doing worker row)

5. Add a merge worker TUI row:
   - Always present, labeled `"merge-worker"`
   - Shows current state: `"idle"`, `"merging PR #N"`, `"polling CI"`, etc.
   - Uses the `on_step` callback in `MergeLoopContext` to update the row

6. Drop `merge_tx` after all doing workers finish to signal the merge worker to exit.

**`tools/gardener/src/worker.rs`**

- Delete `MERGE_PHASE_LOCK`, `merge_phase_lock()`, and `MergePhaseLockGuard`
- Remove the `MergeLockWaiting` and `MergeLockHeld` activity state emissions
- Pass `merge_tx: mpsc::Sender<MergeRequest>` into `execute_task` so the doing worker can send the handoff (alternatively, return `WorkerOutcome` and let the pool handle the send)

### Design Decision: Return vs. Send

Two approaches for the handoff:
- **A) Return `WorkerOutcome` to the pool** — the pool handles the channel send. Simpler, testable, doing worker doesn't need to know about the channel.
- **B) Doing worker sends directly** — lower latency, but couples the worker to the channel.

**Recommendation: A.** The pool already handles all result routing. Keep the doing worker pure (takes input, returns output).

### Success Criteria
- With `--parallelism 3`, doing workers never block on merge
- Merge worker processes tasks FIFO
- `completed` counter increments after merge, not after review
- TUI shows merge worker row with live state updates
- Ctrl+C / interrupt cleanly shuts down both doing workers and merge worker
- Backlog tasks transition: `ready` → `leased` → `in_progress` → `merge_pending` → `in_progress` → `complete`

### Confirmation Gate
Run full integration: `cargo test` + manual test with `--parallelism 2 --quit-after 2` to verify two tasks can overlap (one doing, one merging).

---

## Phase 5: Clean up and harden

### Overview
Remove dead code, handle edge cases, update tests.

### Changes

**`tools/gardener/src/worker.rs`**

- Remove `MERGE_PHASE_LOCK` static, `merge_phase_lock()` fn, `MergePhaseLockGuard` struct and its `Drop` impl
- Remove `WorkerActivityState::MergeLockWaiting` and `MergeLockHeld` variants (and their logging)

**`tools/gardener/src/types.rs`**

- Remove `MergeLockWaiting` and `MergeLockHeld` from `WorkerActivityState` enum if defined there

**`tools/gardener/src/worker_pool.rs`**

- Handle the case where merge worker panics: catch the panic, mark all `merge_pending` tasks as `unresolved`, log the error
- Handle the case where doing workers finish but merge queue still has items: don't drop `merge_tx` until the merge worker has drained its queue (the `recv()` loop handles this naturally — `merge_tx` is dropped when doing workers finish, merge worker drains remaining items, then `recv()` returns `Err` and it exits)

**Tests**

- Unit test: `execute_task_live` returns `HandoffToMerge` on approve verdict
- Unit test: `execute_merge_phase` produces `Complete` summary on successful merge
- Unit test: `mark_merge_pending` + `claim_merge_pending` backlog round-trip
- Unit test: `recover_stale` recovers orphaned `merge_pending` to `ready`
- Integration test: two tasks complete without merge contention

### Success Criteria
- `cargo clippy` clean (no dead code warnings)
- `cargo test` passes
- No references to `MERGE_PHASE_LOCK` remain
- `MergeLockWaiting`/`MergeLockHeld` activity states removed

---

## Testing Strategy

### Automated
- All existing `cargo test` tests pass
- New unit tests for `MergeRequest`/`WorkerOutcome` types
- New unit tests for `execute_merge_phase` (using simulated/mock process runner)
- New backlog store tests for `merge_pending` status transitions
- New migration test (existing migration test framework)

### Manual
- Run with `--parallelism 3 --quit-after 3` and observe:
  - Doing workers never show `MergeLockWaiting`
  - Merge worker row appears and shows progress
  - Tasks complete without being blocked on each other's merges
  - Dashboard stats show `merge_pending` count

### Edge Cases to Verify
- Single task (`--quit-after 1`): doing worker hands off, merge worker merges, clean exit
- Merge failure: merge worker marks task `unresolved`, doing workers unaffected
- CI fix during merge: merge worker runs agent turn successfully
- Interrupt during merge: clean shutdown, `merge_pending` tasks recovered on next run
- Empty merge queue: merge worker blocks on `recv()`, doing workers work normally

## References

- Current merge lock: `worker.rs:67-71`
- Merge loop: `merge_loop.rs:76-406`
- Worker pool dispatch: `worker_pool.rs:51-489`
- Backlog store: `backlog_store.rs:21-52` (statuses), `868-925` (claim), `966-1044` (transitions)
- Worktree lifecycle: `worker.rs:203-209` (create), `worker.rs:1368-1401` (teardown)
