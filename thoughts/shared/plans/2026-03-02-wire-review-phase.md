# Wire worker.rs to use `run_review` from `review_phase.rs`

## Problem

`worker.rs::execute_task_live` duplicates the review phase logic inline instead of calling
`run_review` from `review_phase.rs`. The standalone `review-pr` binary already calls
`run_review`. Key divergence: the review artifact is persisted to different paths (slugified
vs raw task_id), and worker.rs has verdict routing + FSM transitions that the phase module
doesn't perform.

## Current State

`review_phase.rs` exports:
```rust
pub fn run_review(ctx: &ReviewContext<'_>) -> Result<ReviewOutcome, GardenerError>
```
```rust
pub struct ReviewOutcome {
    pub verdict: ReviewVerdict,
    pub suggestions: Vec<String>,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}
```

`worker.rs` (`execute_task_live`, ~line 595):
- Emits `WorkerActivityState::Reviewing`
- Calls `run_agent_turn` directly with `state: WorkerState::Reviewing`
- Pushes result to `logs`
- On `AgentTerminal::Failure`: emits `Failed`, logs `"worker.task.terminal_failure"`,
  returns `WorkerOutcome::Completed(Failed)`
- On success: calls `parse_reviewing_output`, then
  `log_and_persist_review_output(scope, task_id, &identity.worker_id, &reviewing_output)`
  which persists to `.cache/gardener/reviews/{worktree_slug_for_task(task_id)}.json`
- Verdict routing:
  - `NeedsChanges`: logs `"worker.review.needs_changes"`, checks `fsm.review_loops >=
    MAX_REVIEW_LOOPS` (→ Parked), else calls `fsm.on_review_loop_back()`,
    `fsm.transition(Doing)`, emits `WorkerActivityState::Doing`
  - `Approve`: logs `"worker.review.approved"` with suggestions, calls
    `fsm.transition(Merging)`, emits `WorkerActivityState::Merging`, then returns
    `WorkerOutcome::HandoffToMerge(MergeRequest { pr_number, branch, task_id, ... })`

`review_phase.rs` internally:
- Calls `run_agent_turn` with `state: WorkerState::Reviewing`
- Calls `parse_reviewing_output`
- Calls `persist_review_artifact(ctx, &reviewing_output)` which persists to
  `.cache/gardener/reviews/{task_id}.json` (**raw task_id, not slugified**)
- On failure: returns `Err`
- Returns `ReviewOutcome { verdict, suggestions, ... }`

## Key Differences to Resolve

| Concern | Phase module | Worker inline |
|---|---|---|
| Failure signal | `Err(...)` | Returns `Ok(WorkerOutcome::Completed(Failed))` |
| Artifact path | `.cache/gardener/reviews/{task_id}.json` | `.cache/gardener/reviews/{slug(task_id)}.json` |
| Verdict routing | Not performed | `NeedsChanges` vs `Approve` with FSM transitions |
| Review loop cap | Not performed | `fsm.review_loops >= MAX_REVIEW_LOOPS` → Parked |
| Handoff output | Not performed | Returns `WorkerOutcome::HandoffToMerge(MergeRequest)` |
| Activity state events | Not emitted | Emits `Reviewing`, `Parked`, `Doing`, `Merging`, `Failed` |
| Log accumulation | Not performed | `logs.push(log_event_from(...))` |

### Artifact path discrepancy

`review_phase.rs` uses `ctx.task_id` directly. `worker.rs` uses
`worktree_slug_for_task(task_id)` which transforms `manual:testing:foo-bar-123` into
`foo-bar-123`. These write to different files for the same task, which is a pre-existing bug.

**Resolution**: Standardize on the slugified path. Update `persist_review_artifact` in
`review_phase.rs` to apply `worktree_slug_for_task(ctx.task_id)` as the file name.

## Implementation Plan

### Step 1 — Fix artifact path in `review_phase.rs`

In `persist_review_artifact`, replace:
```rust
let filename = format!("{}.json", ctx.task_id);
```
with:
```rust
let filename = format!("{}.json", worktree_slug_for_task(ctx.task_id));
```

Move/export `worktree_slug_for_task` from `worker.rs` to a shared location (e.g.,
`utils.rs` or `types.rs`) so `review_phase.rs` can use it without depending on `worker.rs`.

### Step 2 — Call `run_review` from worker.rs

Replace the inline reviewing section in `execute_task_live`:

```rust
emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Reviewing);

let review_ctx = ReviewContext {
    cfg,
    process_runner,
    scope,
    worktree_path: &req.worktree_path,
    factory: &factory,
    registry: &registry,
    learning_loop: &learning_loop,
    identity: &identity,
    task_summary: &req.task_summary,
    attempt_count: req.attempt_count,
    pr_number,
    branch: &branch,
    task_id,
    on_step: None,
    on_agent_event: None,
};

let review_outcome = match run_review(&review_ctx) {
    Ok(outcome) => outcome,
    Err(e) => {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
        return Ok(WorkerOutcome::Completed(WorkerRunSummary {
            final_state: WorkerState::Failed,
            failure_reason: Some(e.to_string()),
            ...
        }));
    }
};

// Verdict routing stays in worker.rs
match review_outcome.verdict {
    ReviewVerdict::NeedsChanges => {
        append_run_log("info", "worker.review.needs_changes", json!({...}));
        if fsm.review_loops >= MAX_REVIEW_LOOPS {
            // Parked path
        } else {
            fsm.on_review_loop_back();
            fsm.transition(WorkerState::Doing);
            emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Doing);
        }
    }
    ReviewVerdict::Approve => {
        append_run_log("info", "worker.review.approved", json!({...}));
        fsm.transition(WorkerState::Merging);
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Merging);
        return Ok(WorkerOutcome::HandoffToMerge(MergeRequest { pr_number, branch, task_id, ... }));
    }
}
```

Remove `log_and_persist_review_output` from `worker.rs` once `review_phase.rs` handles
persistence correctly (after Step 1).

### Step 3 — Remove `log_and_persist_review_output` from worker.rs

Once `review_phase.rs::persist_review_artifact` uses the slugified path, the duplicate
`log_and_persist_review_output` function in `worker.rs` can be deleted.

### Step 4 — Tests

Verify:
- Artifact is written to the slugified path from both worker.rs and the standalone binary
- `NeedsChanges` loop cap (Parked) still triggers at `MAX_REVIEW_LOOPS`
- `Approve` still returns `HandoffToMerge` with the correct `MergeRequest`

## Files Changed

- `tools/gardener/src/review_phase.rs` — fix artifact path, use slugified task_id
- `tools/gardener/src/worker.rs` — replace inline review logic with `run_review` call,
  remove `log_and_persist_review_output`
- `tools/gardener/src/utils.rs` (or `types.rs`) — export `worktree_slug_for_task` for
  shared use
