# Wire worker.rs to use `run_understand` from `understand_phase.rs`

## Problem

`worker.rs::execute_task_live` duplicates the understand phase logic inline instead of calling
`run_understand` from `understand_phase.rs`. The standalone `understand` binary already calls
`run_understand` correctly. Any bug fixed in one place must be manually mirrored in the other —
as demonstrated by the `Blocked` PR remediation bug that had to be patched in two files.

## Current State

`understand_phase.rs` exports:
```rust
pub fn run_understand(ctx: &UnderstandContext<'_>) -> Result<UnderstandOutcome, GardenerError>
```

`worker.rs` (`execute_task_live`, ~line 256):
- Emits `WorkerActivityState::Understand`
- Calls `run_agent_turn` directly with `state: WorkerState::Understand`
- On `AgentTerminal::Failure`: emits `WorkerActivityState::Failed`, returns `WorkerOutcome::Completed(Failed)`
- On success: calls `parse_understand_output`, then `fsm.apply_understand(...)`, logs `"worker.task.classified"`

`understand_phase.rs` internally:
- Calls `run_agent_turn` with `state: WorkerState::Understand`
- Calls `parse_understand_output` (including the keyword-match fallback)
- On failure: returns `Err(GardenerError::Process(...))`
- Returns `UnderstandOutcome { category, reasoning, prompt_version, context_manifest_hash }`

## Key Differences to Resolve

| Concern | Phase module | Worker inline |
|---|---|---|
| Failure signal | `Err(GardenerError::Process(...))` | Returns `Ok(WorkerOutcome::Completed(Failed))` |
| FSM transition | Not performed | `fsm.apply_understand(&understand, attempt_count > 1)` |
| Activity state events | Not emitted | Emits `Understand` before, `Failed` on failure |
| Log accumulation | Not performed | `logs.push(log_event_from(&result, WorkerState::Understand))` |
| "classified" log | Not emitted | `"worker.task.classified"` with task type, reasoning, worktree, branch |
| `on_step` callback | Fires step callbacks | Worker passes `on_event: None` — no steps wired |

Worker.rs owns FSM transitions, activity state events, and log accumulation. Those stay in
worker.rs. The understand phase logic (agent turn + parse + fallback) moves to the phase module.

## Implementation Plan

### Step 1 — Call `run_understand` from worker.rs

Replace the inline `run_agent_turn` + `parse_understand_output` block in `execute_task_live`
with a call to `run_understand`:

```rust
emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Understand);

let understand_ctx = UnderstandContext {
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
    on_step: None,
    on_agent_event: None,
};

let understand_outcome = match run_understand(&understand_ctx) {
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
```

Remove the inline agent turn call, `parse_understand_output` call, and associated log emission.
Keep `fsm.apply_understand(...)` and `"worker.task.classified"` log in worker.rs.

The `logs.push(...)` accumulation currently uses `log_event_from(&understand_result, ...)` where
`understand_result` is the raw `AgentTurnOutput`. Since `run_understand` no longer exposes the
raw turn output, either:
- (a) Remove the understand step from `logs` accumulation (it's already captured via OTEL), or
- (b) Add a `raw_turn: AgentTurnOutput` field to `UnderstandOutcome`

Option (a) is simpler and consistent with how the standalone `understand` binary works.

### Step 2 — Remove dead imports

Remove any `understand_phase` imports in `worker.rs` that are no longer needed after the
refactor (e.g. `classify_task` if it was only used as a fallback now handled inside
`run_understand`).

### Step 3 — Tests

Confirm existing tests pass. Add a test that verifies the `understand` phase in
`execute_task_live` produces a `worker.task.classified` event with the expected task type when
`run_understand` succeeds.

## Files Changed

- `tools/gardener/src/worker.rs` — replace inline understand logic with `run_understand` call
- `tools/gardener/src/understand_phase.rs` — no changes expected
