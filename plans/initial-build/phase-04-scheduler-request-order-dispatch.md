## Phase 4: Scheduler + Request-Order Dispatch
Context: [Vision](./00-gardener-vision.md) | [Shared Foundation](./01-shared-foundation.md)
### Changes Required
- Add scheduler engine:
  - `tools/gardener/src/scheduler.rs`
  - `tools/gardener/src/worker_pool.rs`.
- Add phase-local worker stub mode for scheduler validation:
  - `worker_mode = "stub_complete"` in phase 4 fixture causes claimed tasks to transition `leased -> complete` without FSM dependency.
- Workers request tasks through explicit pull channel.
- Represent waiting workers explicitly as FIFO queue of request handles (`VecDeque<WorkRequest>`), served strictly by arrival order.
- Add redundant completion observers:
  - worker completion hook
  - scheduler reconciliation sweep
  - periodic watchdog reconciliation.
- Scheduler serves tasks in strict order:
  - priority (`P0` before `P1` before `P2`)
  - then FIFO by `last_updated`
  - then request order.
- Enforce lease timeout, heartbeat refresh, and deterministic requeue policy for crashed/hung workers.
- Use explicit defaults from shared foundation:
  - lease timeout `900s`
  - heartbeat interval `15s`
  - starvation threshold `180s`
  - reconcile interval `30s`.
- Update projection caches (`tasks`, `worker_state`) from immutable events with monotonic `last_event_id` checks.
- Add scheduler metrics:
  - claim latency
  - queue depth by priority
  - requeue count
  - starvation watchdog.

### Success Criteria
- Request-order fairness is test-proven.
- FIFO worker-request servicing is test-proven via request-id traces.
- Phase 4 E2E validation is independent of Phase 5 FSM by using `stub_complete` worker mode.
- No double-assignment under stress.
- Orchestrator survives worker crash and resumes queue correctly.
- Starvation watchdog detects and escalates blocked queue progression.
- Observer races are idempotent and cannot leave tasks permanently stranded in non-terminal states.
- Projection drift is detected and repaired deterministically.
- Periodic projection reconciliation (`scheduler.reconcile_interval_seconds`) and entity-replay/full-rebuild fallback are test-proven.

### Phase Validation Gate (Mandatory)
- Run: `cargo test -p gardener --all-targets`
- Run: `cargo llvm-cov -p gardener --all-targets --summary-only` (must report 100.00% lines for current `tools/gardener/src/**` code at this phase).
- Run E2E binary smoke: `scripts/brad-gardener --parallelism 3 --target 3 --config tools/gardener/tests/fixtures/configs/phase04-scheduler-stub.toml`.

### Autonomous Completion Rule
- Continue directly to the next phase only after all success criteria and this phase validation gate pass.
- Do not wait for manual approval checkpoints.
