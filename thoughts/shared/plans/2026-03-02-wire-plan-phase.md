# Wire worker.rs to use `run_plan` from `plan_phase.rs`

## Problem

`worker.rs::execute_task_live` duplicates the planning phase logic inline instead of calling
`run_plan` from `plan_phase.rs`. The standalone `plan` binary already calls `run_plan`
correctly. Same dual-maintenance problem as all the other phase modules.

## Current State

`plan_phase.rs` exports:
```rust
pub fn run_plan(ctx: &PlanContext<'_>) -> Result<PlanOutcome, GardenerError>
```

`worker.rs` (`execute_task_live`, ~line 307):
- Only entered when `fsm.state == WorkerState::Planning` (set by `fsm.apply_understand`)
- Emits `WorkerActivityState::Planning`
- Calls `run_agent_turn` directly with `state: WorkerState::Planning`
- On `AgentTerminal::Failure`: emits `WorkerActivityState::Failed`, logs
  `"worker.task.terminal_failure"` with `"state": "planning"`, returns
  `WorkerOutcome::Completed(Failed)`
- On success: calls `fsm.transition(WorkerState::Doing)`

`plan_phase.rs` internally:
- Calls `run_agent_turn` with `state: WorkerState::Planning`
- Emits `"plan_phase.started"` log
- On `AgentTerminal::Failure`: returns `Err(GardenerError::Process(...))`
- Returns `PlanOutcome { prompt_version, context_manifest_hash }`

## Key Differences to Resolve

| Concern | Phase module | Worker inline |
|---|---|---|
| Failure signal | `Err(GardenerError::Process(...))` | Returns `Ok(WorkerOutcome::Completed(Failed))` |
| FSM transition | Not performed | `fsm.transition(WorkerState::Doing)` |
| Activity state events | Not emitted | Emits `Planning` before, `Failed` on failure |
| Log accumulation | Not performed | `logs.push(log_event_from(&result, WorkerState::Planning))` |
| "started" log | `"plan_phase.started"` | No equivalent |
| Conditional entry | Always runs if called | Gated on `fsm.state == WorkerState::Planning` |

The FSM gate and transition stay in worker.rs. The agent turn + failure handling moves to
the phase module.

## Implementation Plan

### Step 1 — Call `run_plan` from worker.rs

Replace the inline `run_agent_turn` block in the planning section of `execute_task_live`:

```rust
// Planning is only entered when the understand phase determined it's needed
if fsm.state == WorkerState::Planning {
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Planning);

    let plan_ctx = PlanContext {
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

    if let Err(e) = run_plan(&plan_ctx) {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
        return Ok(WorkerOutcome::Completed(WorkerRunSummary {
            final_state: WorkerState::Failed,
            failure_reason: Some(e.to_string()),
            ...
        }));
    }

    fsm.transition(WorkerState::Doing);
}
```

The `logs.push(...)` for the planning turn can be dropped for the same reason as understand —
the raw turn is already in OTEL. If log accumulation is important to preserve, add
`raw_turn: AgentTurnOutput` to `PlanOutcome`.

### Step 2 — Remove dead inline code and imports

Remove any `plan_phase`-related imports that are no longer needed in worker.rs after the
refactor.

### Step 3 — Tests

Confirm existing tests pass. Verify that a task that requires planning (TaskCategory requiring
a plan step) still transitions FSM through `Planning → Doing` correctly.

## Files Changed

- `tools/gardener/src/worker.rs` — replace inline plan logic with `run_plan` call
- `tools/gardener/src/plan_phase.rs` — no changes expected
