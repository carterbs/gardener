# Wire worker.rs to use `run_git_push` from `git_phase.rs`

## Problem

`worker.rs::execute_task_live` duplicates the gitting phase logic inline instead of calling
`run_git_push` from `git_phase.rs`. The standalone `git-push` binary already calls
`run_git_push`. Key divergence: the phase module calls `git.commit_all` at its own start,
but worker.rs already did a safety-net commit at the end of the Doing phase, making a second
commit unnecessary and potentially creating an empty commit.

## Current State

`git_phase.rs` exports:
```rust
pub fn run_git_push(ctx: &GitPushContext<'_>) -> Result<GitPushOutcome, GardenerError>
```
```rust
pub struct GitPushOutcome {
    pub pr_number: u64,
    pub pr_url: String,
}
```

`worker.rs` (`execute_task_live`, ~line 445):
- Emits `WorkerActivityState::Gitting`, logs `"worker.gitting.deterministic.started"`
- Loops up to `MAX_GITTING_REMEDIATION` push attempts:
  - On push failure (non-final): logs `"worker.gitting.deterministic.push_failed"`, calls
    `learning_loop.ingest_failure(WorkerState::Gitting, ...)`, emits
    `WorkerActivityState::GittingRemediation`, runs `run_agent_turn`, pushes to `logs`,
    checks agent failure, calls `git.commit_all("fix: gitting remediation")`
  - On loop exhaustion: logs `"worker.gitting.deterministic.exhausted"`, emits `Failed`,
    returns `WorkerOutcome::Completed(Failed)`
  - On push success: logs `"worker.gitting.deterministic.succeeded"`, breaks
- Emits `WorkerActivityState::PrCreating`, runs PR creation agent turn
- Finds PR via `gh.find_pr_for_branch`, discards url: `let (number, _url) = ...`
- Logs `"worker.gitting.deterministic.pr_created"` with pr_number

`git_phase.rs` internally:
- **Calls `git.commit_all(ctx.commit_message)` at the start** (differs from worker.rs which
  already committed during Doing)
- Loops up to `MAX_GITTING_REMEDIATION` push attempts
- On each failure: logs `"git_phase.push.remediation"`, runs agent turn (no `ingest_failure`,
  no `logs.push`), calls `git.commit_all("fix: gitting remediation")`
- On exhaustion: returns `Err(GardenerError::Process("gitting failed..."))`
- On success: logs `"git_phase.push.succeeded"`
- Runs PR creation agent turn, finds PR, returns `GitPushOutcome { pr_number, pr_url }`

## Key Differences to Resolve

| Concern | Phase module | Worker inline |
|---|---|---|
| Initial commit | `git.commit_all(ctx.commit_message)` at start | Already committed at end of Doing; no commit at gitting start |
| `learning_loop.ingest_failure` | Not called | Called on each push failure |
| Failure signal | `Err(...)` | Returns `Ok(WorkerOutcome::Completed(Failed))` |
| Activity state events | Not emitted | Emits `Gitting`, `GittingRemediation`, `PrCreating`, `Failed` |
| Log accumulation | Not performed | `logs.push(log_event_from(...))` for each remediation turn |
| PR URL | Returned in `GitPushOutcome` | Discarded at call site |
| Exhaustion log event name | `"gitting failed after..."` in error string | `"worker.gitting.deterministic.exhausted"` JSON log |

The initial commit difference is the most important. The phase module must be told whether
to perform an initial commit or skip it (the worker already committed).

## Implementation Plan

### Step 1 — Add `skip_initial_commit: bool` to `GitPushContext`

```rust
pub struct GitPushContext<'a> {
    // ... existing fields ...
    /// When true, skip the initial `git.commit_all` at phase start.
    /// Use when the caller has already committed (e.g. worker.rs safety-net commit).
    pub skip_initial_commit: bool,
}
```

In `run_git_push`, gate the commit:
```rust
if !ctx.skip_initial_commit {
    ctx.git.commit_all(ctx.commit_message)?;
}
```

The standalone `git-push` binary passes `skip_initial_commit: false` (current behavior unchanged).
Worker.rs passes `skip_initial_commit: true`.

### Step 2 — Call `run_git_push` from worker.rs

Replace the inline gitting section in `execute_task_live`:

```rust
emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Gitting);
append_run_log("info", "worker.gitting.deterministic.started", json!({...}));

let git_ctx = GitPushContext {
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
    branch: &branch,
    commit_message: &format!("feat: {}", req.task_summary),
    skip_initial_commit: true,
    on_step: None,
    on_agent_event: None,
};

let git_outcome = match run_git_push(&git_ctx) {
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

let pr_number = git_outcome.pr_number;
```

Note: `learning_loop.ingest_failure` on push failures and the per-remediation `logs.push`
are currently inline. These are either:
- (a) dropped in favor of OTEL (acceptable — the phase module already logs via `append_run_log`)
- (b) plumbed into the phase module via `on_step` callbacks

Option (a) is simpler for now.

### Step 3 — Activity state events

The phase module doesn't emit `WorkerActivityState` events. Two options:
- (a) Add `on_step` callbacks to the push loop in the phase module that worker.rs uses to
  emit `GittingRemediation` and `PrCreating` events
- (b) Accept that those granular events are dropped; only `Gitting` and completion/failure
  events remain visible

Option (b) reduces visibility in the dashboard. Option (a) is the right long-term approach
but adds complexity. Document the tradeoff and decide before implementation.

### Step 4 — Tests

Verify:
- Push failure → remediation → success path still works
- `skip_initial_commit: false` (standalone binary) still commits before push
- `skip_initial_commit: true` (worker) does not create a duplicate empty commit

## Files Changed

- `tools/gardener/src/git_phase.rs` — add `skip_initial_commit` field to `GitPushContext`
- `tools/gardener/src/worker.rs` — replace inline gitting logic with `run_git_push` call
- `tools/gardener/src/bin/git_push.rs` — pass `skip_initial_commit: false` (no change in behavior)
