# Wire worker.rs to use `run_merge_loop` from `merge_loop.rs`

## Problem

`worker.rs::execute_merge_phase` duplicates the merge phase logic inline instead of calling
`run_merge_loop` from `merge_loop.rs`. The standalone `merge-pr` binary already calls
`run_merge_loop`. This is the phase that caused the `Blocked` PR bug — a fix had to be
applied in two separate files (`merge_loop.rs` and `worker.rs`). The merge phase also has
the most substantial differences due to pre/post validation, friction analysis, and teardown
that exist only in `execute_merge_phase`.

## Current State

`merge_loop.rs` exports:
```rust
pub fn run_merge_loop(ctx: &mut MergeLoopContext<'_>) -> Result<MergeLoopOutcome, GardenerError>
```
```rust
pub enum MergeLoopOutcome {
    Merged { sha: String },
    Failed { reason: String },
}
```

`worker.rs::execute_merge_phase` (~line 827):
1. Constructs `AdapterFactory`, `PromptRegistry`, `LearningLoop`, `WorkerIdentity`,
   `GhClient`, `GitClient`, `WorktreeClient`
2. **Pre-merge validation**: calls `run_repo_validation_with_quality_guard` before each
   `merge_pr` attempt; returns `WorkerRunSummary(Failed)` if it fails
3. **Inline merge loop**: structurally parallel to `run_merge_loop` but with:
   - `WorkerActivityState` events (`MergePolling`, `MergeFromMain`, `MergeRemediation`,
     `CiFailureRemediation`, `PostMergeValidation`, `Teardown`, `Complete`, `Failed`)
   - `logs` accumulation for each agent turn
   - `merge_output: MergingOutput { merged, merge_sha }` tracking across loop iterations
4. **Post-merge validation**: calls `run_repo_validation_with_quality_guard` after merge
5. **Friction analysis**: calls `friction_analysis::run_friction_analysis`, upserts findings
   to `BacklogStore`
6. **Teardown**: calls `teardown_after_completion` (worktree cleanup, pull_main)
7. Returns `WorkerRunSummary { final_state: Complete, teardown: Some(...), ... }`

`merge_loop.rs` internally:
- Polls mergeability
- State machine: Clean/HasHooks → merge, Behind/Dirty → rebase+push, Unstable/Blocked →
  fetch failed checks + agent remediation
- Returns `MergeLoopOutcome::Merged { sha }` or `MergeLoopOutcome::Failed { reason }`
- **No pre/post validation, no friction analysis, no teardown, no activity state events**

## Key Differences to Resolve

| Concern | `run_merge_loop` | `execute_merge_phase` |
|---|---|---|
| Return type | `MergeLoopOutcome` enum | `WorkerRunSummary` |
| Pre-merge validation | Not performed | `run_repo_validation_with_quality_guard` before `merge_pr` |
| Post-merge validation | Not performed | `run_repo_validation_with_quality_guard` after merge |
| Friction analysis | Not performed | `run_friction_analysis` + `BacklogStore` upsert |
| Teardown | Not performed | `teardown_after_completion` |
| Activity state events | Not emitted | Full set emitted |
| `logs` accumulation | Not performed | Agent turns accumulated |
| `learning_loop` borrow | `&mut LearningLoop` in context | Fresh `LearningLoop` per execution |
| Pre-merge validation location | N/A | Inside the `Clean|HasHooks` match arm (before `merge_pr`) |

## Implementation Plan

The pre/post validation, friction analysis, and teardown are the reasons `execute_merge_phase`
can't be replaced wholesale by `run_merge_loop`. The right architecture is:

```
execute_merge_phase:
  1. Pre-setup (factory, registry, clients)
  2. → run_merge_loop (core loop: poll, state machine, agent remediation)
  3. Post-merge validation
  4. Friction analysis
  5. Teardown
  6. Return WorkerRunSummary
```

### Step 1 — Move pre-merge validation into `run_merge_loop`

Pre-merge validation (`run_repo_validation_with_quality_guard`) currently runs before each
`merge_pr` call in the `Clean|HasHooks` arm. Move it inside `run_merge_loop`:

```rust
// In merge_loop.rs, Clean|HasHooks arm:
(Mergeable::Mergeable, MergeStateStatus::Clean | MergeStateStatus::HasHooks) => {
    if let Some(validation_fn) = ctx.pre_merge_validation {
        if let Err(e) = (validation_fn)() {
            step(ctx, "VALIDATE", &format!("pre-merge validation failed: {e}"));
            return Ok(MergeLoopOutcome::Failed {
                reason: format!("pre-merge validation failed: {e}"),
            });
        }
    }
    // ... existing merge_pr call
}
```

Use a closure field on `MergeLoopContext`:
```rust
pub struct MergeLoopContext<'a> {
    // ... existing fields ...
    pub pre_merge_validation: Option<&'a dyn Fn() -> Result<(), GardenerError>>,
}
```

The standalone `merge-pr` binary passes `pre_merge_validation: None`.
`execute_merge_phase` passes a closure that calls `run_repo_validation_with_quality_guard`.

### Step 2 — Call `run_merge_loop` from `execute_merge_phase`

Replace the inline merge loop in `execute_merge_phase`:

```rust
// Construct GhClient, GitClient as before
let gh = GhClient::new(&runner, &worktree_path);
let git = GitClient::new(&runner, &worktree_path);

let merge_outcome = run_merge_loop(&mut MergeLoopContext {
    cfg: &cfg,
    process_runner: &runner,
    scope: &scope,
    worktree_path: &worktree_path,
    factory: &factory,
    registry: &registry,
    learning_loop: &mut learning_loop,
    identity: &identity,
    task_summary: &req.task_summary,
    attempt_count: req.attempt_count,
    gh: &gh,
    git: &git,
    branch: &req.branch,
    pr_number: req.pr_number,
    validation_command: &cfg.validation.command,
    pre_merge_validation: Some(&|| run_repo_validation_with_quality_guard(...)),
    on_step: Some(&|label, detail| { /* emit WorkerActivityState events */ }),
    on_agent_event: None,
})?;
```

The `on_step` callback is how `execute_merge_phase` gets visibility into the loop state to
emit `WorkerActivityState` events.

### Step 3 — Keep post-loop logic in `execute_merge_phase`

After `run_merge_loop` returns:

```rust
let merge_sha = match merge_outcome {
    MergeLoopOutcome::Merged { sha } => sha,
    MergeLoopOutcome::Failed { reason } => {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
        return Ok(WorkerRunSummary { final_state: Failed, failure_reason: Some(reason), ... });
    }
};

// Post-merge validation
emit_worker_activity_state(worker_id, task_id, WorkerActivityState::PostMergeValidation);
if let Err(e) = run_repo_validation_with_quality_guard(...) {
    // log worker.merging.post_validation_failed, return Failed
}

// Friction analysis
run_friction_analysis(...);

// Teardown
emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Teardown);
let teardown = teardown_after_completion(...);

emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Complete);
append_run_log("info", "worker.merge_phase.complete", json!({...}));
Ok(WorkerRunSummary { final_state: Complete, teardown: Some(teardown), ... })
```

### Step 4 — Activity state events via `on_step`

Add a standardized step label scheme to `merge_loop.rs` steps so `execute_merge_phase` can
map them to `WorkerActivityState` variants:

```
"POLL"       → MergePolling
"MERGE"      → (no change needed, merge is brief)
"REMEDIATE"  → MergeRemediation / CiFailureRemediation / MergeFromMain
```

The `on_step` callback in `execute_merge_phase` translates label strings to
`emit_worker_activity_state_with(...)` calls.

### Step 5 — Remove duplicate merge loop from `worker.rs`

Once `execute_merge_phase` delegates to `run_merge_loop`, delete:
- The inline merge loop body (~500 lines)
- `worker_merge_main_and_push` helper function
- The `merge_polling_block_reason` function (already in worker.rs, not needed if
  `merge_loop.rs` handles it)

Keep in `execute_merge_phase`:
- Client/factory/registry construction
- Post-merge validation
- Friction analysis
- Teardown
- `WorkerRunSummary` construction

### Step 6 — Tests

Verify:
- Pre-merge validation failure returns `Failed` before any merge attempt
- Post-merge validation failure returns `Failed` after merge
- Friction analysis findings are upserted to the backlog
- Blocked + failed checks → agent remediation path (the fix we just shipped) works
  in the wired-up path

## Files Changed

- `tools/gardener/src/merge_loop.rs` — add `pre_merge_validation` closure to
  `MergeLoopContext`, move pre-merge validation into `Clean|HasHooks` arm
- `tools/gardener/src/worker.rs` — replace inline merge loop with `run_merge_loop` call,
  delete `worker_merge_main_and_push`, `merge_polling_block_reason`
- `tools/gardener/src/bin/merge_pr.rs` — pass `pre_merge_validation: None`
