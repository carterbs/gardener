# Wire worker.rs to use `run_do` from `do_phase.rs`

## Problem

`worker.rs::execute_task_live` duplicates the doing phase logic inline instead of calling
`run_do` from `do_phase.rs`. The standalone `do-task` binary already calls `run_do`.
This phase has the most meaningful divergence between the two paths: worker.rs has a git
fallback that `do_phase.rs` lacks.

## Current State

`do_phase.rs` exports:
```rust
pub fn run_do(ctx: &DoContext<'_>) -> Result<DoOutcome, GardenerError>
```
```rust
pub struct DoOutcome {
    pub summary: String,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}
```

`worker.rs` (`execute_task_live`, ~line 348):
- Captures `pre_doing_sha = git.head_sha()?.unwrap_or_default()` before calling agent
- Emits `WorkerActivityState::Doing`
- Calls `run_agent_turn` directly with `state: WorkerState::Doing`
- Pushes result to `logs`
- On `AgentTerminal::Failure`: returns `WorkerOutcome::Completed(Failed)`
- Calls `parse_doing_output`
- **Git fallback** (absent from phase module): if `parse_doing_output` fails, calls
  `git.commits_since(&pre_doing_sha)`. If commits exist, uses first commit subject as summary
  with `"worker.doing.payload_fallback_to_git"` log. Only if no commits → Failed.
- After successful output: calls `fsm.on_doing_turn_completed()`, checks `fsm.state ==
  WorkerState::Parked` (returns early if so), calls `git.commit_all(&fallback_commit_message(...))`,
  then `fsm.transition(WorkerState::Gitting)`

`do_phase.rs` internally:
- Calls `run_agent_turn` with `state: WorkerState::Doing`
- Calls `parse_doing_output`; on parse failure returns `Err` immediately (no git fallback)
- On agent failure returns `Err`
- Returns `DoOutcome { summary, prompt_version, context_manifest_hash }`

## Key Differences to Resolve

| Concern | Phase module | Worker inline |
|---|---|---|
| Failure signal | `Err(GardenerError::Process(...))` | Returns `Ok(WorkerOutcome::Completed(Failed))` |
| Parse failure fallback | None — returns `Err` | Git commit history fallback via `git.commits_since` |
| Pre-doing SHA capture | Not performed | Captures `pre_doing_sha` for git fallback |
| FSM transitions | Not performed | `fsm.on_doing_turn_completed()`, Parked check, `transition(Gitting)` |
| Safety-net commit | Not performed | `git.commit_all(&fallback_commit_message(...))` |
| Parked early exit | Not performed | Checks `fsm.state == Parked`, returns early |
| Activity state events | Not emitted | Emits `Doing`, `Commit`, `Failed` |
| Log accumulation | Not performed | `logs.push(log_event_from(...))` |

The git fallback is the most significant difference. It represents real behavior that should
not be silently dropped during migration.

## Implementation Plan

### Step 1 — Add git fallback support to `do_phase.rs`

The parse fallback belongs in the phase module so all callers benefit. Add an optional
`git: Option<&GitClient>` field to `DoContext`, or expose a `pre_sha: Option<&str>` that the
phase module uses internally:

```rust
pub struct DoContext<'a> {
    // ... existing fields ...
    /// If provided, used for git-commit fallback when payload parse fails
    pub git: Option<&'a GitClient<'a>>,
}
```

Inside `run_do`, after `parse_doing_output` fails:
```rust
if let Some(git) = ctx.git {
    let pre_sha = git.head_sha()?.unwrap_or_default(); // captured before agent turn
    let commits = git.commits_since(&pre_sha).unwrap_or_default();
    if let Some(first) = commits.first() {
        append_run_log("warn", "worker.doing.payload_fallback_to_git", json!({...}));
        // use first commit subject as summary
        return Ok(DoOutcome { summary: first.subject.clone(), ... });
    }
}
return Err(GardenerError::Process("doing output parse failed".to_string()));
```

Note: `pre_sha` needs to be captured *before* the agent turn. This requires capturing it
at context construction time or passing it in. Easiest: capture it in worker.rs before
constructing `DoContext` and pass it as an extra field `pre_doing_sha: Option<String>`.

### Step 2 — Call `run_do` from worker.rs

Replace the inline doing section in `execute_task_live`:

```rust
emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Doing);

let pre_doing_sha = git.head_sha()?.unwrap_or_default();
let do_ctx = DoContext {
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
    git: Some(&git),
    pre_doing_sha: Some(pre_doing_sha),
    on_step: None,
    on_agent_event: None,
};

let _do_outcome = match run_do(&do_ctx) {
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

// FSM + commit stay in worker.rs
fsm.on_doing_turn_completed();
if fsm.state == WorkerState::Parked {
    // emit Parked, return early
}
emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Commit);
git.commit_all(&fallback_commit_message(&req.task_summary))?;
fsm.transition(WorkerState::Gitting);
```

### Step 3 — Tests

Verify:
- Git fallback path still fires when parse fails but commits exist
- Parked early exit still works
- Safety-net commit still runs after successful doing

## Files Changed

- `tools/gardener/src/do_phase.rs` — add optional git fallback support
- `tools/gardener/src/worker.rs` — replace inline doing logic with `run_do` call
- `tools/gardener/src/bin/do_task.rs` — pass `git: None` (standalone binary has no git fallback need)
