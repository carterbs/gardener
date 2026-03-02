## Fix Worker Flow Mismatch: handoff-to-merge still showing Understand

### Context
Observed behavior in runtime UI shows tasks in `Understand` flow while command history indicates `v1-reviewing`, handoff to merge worker, and PR claim (`handoff to merge`, `claimed`) in the same row.

### Root Cause Hypotheses
- `normalize_worker_state()` maps several pre-handoff lifecycle states to `understand`.
- In handoff path, UI row state can be set to `handoff` and later overwritten by asynchronous log/event replay.
- `append_worker_state_events()` applies `worker.activity.state_changed` events directly without run/task recency guarding, so stale state transitions can regress chip rendering.

### Implementation Plan
1. Preserve handoff intent in UI normalization
   - File: `tools/gardener/src/tui.rs` (`normalize_worker_state`).
   - Add explicit handling so `handoff` resolves to `merging`.
   - Ensure phase rendering for rows in transition cannot visually downgrade below `merging` when merge handoff command history is present.

2. Make asynchronous state replay safer
   - File: `tools/gardener/src/worker_pool.rs` (`append_worker_state_events`, call sites).
   - Add a per-row recency/epoch guard before state overwrite:
     - ignore `state_changed` events that are older than row’s last authoritative transition.
     - ignore events from stale `run_id`/task epoch when available.
   - Keep explicit handoff state updates in `WorkerOutcome::HandoffToMerge` as source of truth.

3. Improve explicit handoff-to-merge state assignment
   - File: `tools/gardener/src/worker_pool.rs` (`WorkerOutcome::HandoffToMerge`).
   - Mark merged rows as `merging` (not `handoff`) during handoff so command-chain chips and status text are merge-consistent.
   - Keep status log text for `handoff` in command stream, but avoid demoting the state chip.

4. Prevent `claimed`-to-merge regressions
   - File: `tools/gardener/src/worker_pool.rs` plus `tools/gardener/src/worker.rs` transition points.
   - Ensure post-handoff reclaim/reassignment does not overwrite an in-flight merge handoff row without a task-id change boundary.
   - Add strict monotonicity in phase updates (new phase cannot move backward unless terminal/restart path).

5. Add test coverage (lightweight, deterministic)
   - File: `tools/gardener/src/tui.rs` tests.
     - `normalize_worker_state("handoff") -> "merging"`.
     - ensure `claimed`-like states do not override an active merge handoff indicator.
   - File: `tools/gardener/src/worker_pool.rs` tests.
     - simulate handoff + later stale `state_changed` and assert row remains in merge-visible state.
     - simulate immediate handoff then reclaim and confirm guarded ordering keeps display merge-consistent.

### Acceptance Criteria
- Task rows with command evidence of handoff/merge chips show `Merging` path, not `Understand`.
- No visible state regression to earlier phases after handoff when stale/older log events arrive.
- `handoff` no longer appears as an unknown/fallback phase in UI flow rendering.
- Phase rendering remains stable under rapid task lifecycle transitions.
- Existing merge/review handoff behavior remains unchanged aside from improved phase display.

### Notes
- No functional behavior change to merge logic is intended in this phase; this is a state-source-of-truth and rendering consistency fix.
