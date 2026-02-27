# Half-Implemented & Unwired Features in gardener

**Date:** 2026-02-27
**Scope:** `tools/gardener/src/`
**Purpose:** Catalog features that are defined/scaffolded but not wired up or completed

---

## 1. Full Stubs (functions that return defaults and do nothing)

### `reconcile_worktrees()` — `worktree_audit.rs:7-9`
```rust
pub fn reconcile_worktrees() -> WorktreeAuditSummary {
    WorktreeAuditSummary::default()
}
```
Always returns `{stale_found: 0, stale_fixed: 0}`. Called in `startup.rs:97`. The real implementation would inspect the filesystem/git for stale worktrees. `WorktreeClient` in `worktree.rs` has the relevant methods but is never imported by production code.

### `reconcile_open_prs()` — `pr_audit.rs:7-9`
```rust
pub fn reconcile_open_prs() -> PrAuditSummary {
    PrAuditSummary::default()
}
```
Always returns `{collisions_found: 0, collisions_fixed: 0}`. Called in `startup.rs:98`. The `upgrade_unmerged_collision_priority` function in `gh.rs` exists and is tested, but `reconcile_open_prs()` never calls it.

---

## 2. Hotkey Actions Registered But Not Implemented

**`worker_pool.rs:136-144`**
```rust
Some(AppHotkeyAction::Retry) => {
    terminal.write_line("retry requested (not yet implemented)")?;
}
Some(AppHotkeyAction::ReleaseLease) => {
    terminal.write_line("release-lease requested (not yet implemented)")?;
}
Some(AppHotkeyAction::ParkEscalate) => {
    terminal.write_line("park/escalate requested (not yet implemented)")?;
}
```
Keys `r`, `l`, `p` are in `DASHBOARD_BINDINGS`, enforced by lint tests, have `HotkeyAction` enum variants, route correctly — but the handler bodies just print a string. No backlog mutation, lease release, or escalation logic exists.

---

## 3. Data Computed or Set But Immediately Discarded

### `doing_payload` discarded — `worker.rs:256-272`
```rust
let doing_payload = DoingOutput {
    summary: "implementation complete".to_string(),
    files_changed: vec!["src/lib.rs".to_string()],
};
// ...
let _ = doing_payload;  // discarded
```
Hardcoded `DoingOutput` constructed in `execute_task_simulated`, never used.

### `_parsed` discarded — `worker.rs:485-493`
Parse result from a synthetic envelope inside `prepare_prompt` is immediately prefixed `_`. Validates the parser but has no effect on the returned `PreparedPrompt`.

### `let _ = WorkerState::Seeding` — `seed_runner.rs:56`
A bare discard to suppress compiler warnings. `WorkerState::Seeding` never appears in the FSM transition table — `fsm.rs:135` puts it in the terminal/sink set.

---

## 4. Struct Fields Always Hardcoded / Never Read in Production

### `TeardownReport` — `worker.rs:566-574`
```rust
fn teardown_after_completion(output: &MergingOutput) -> TeardownReport {
    TeardownReport {
        merge_verified: output.merged,
        session_torn_down: true,   // no actual session teardown
        sandbox_torn_down: true,   // no actual sandbox teardown
        worktree_cleaned: true,    // no actual worktree cleanup
        state_cleared: true,       // no actual state clearing
    }
}
```
The four boolean fields beyond `merge_verified` are unconditional. No real teardown happens.

### `AdapterContext::knowledge_refs` — `agent/mod.rs:21`
Every call site passes `knowledge_refs: vec![]`. Neither `ClaudeAdapter` nor `CodexAdapter` reads this field. Declared but never wired up to the underlying CLI invocations.

### `AdapterContext::cancel_requested` — `claude.rs:78-83`, `codex.rs:88-93`
Cancel check exists in adapters (post-spawn, pre-wait), but every production call site passes `cancel_requested: false`. Only set to `true` in tests. A single synchronous poll rather than a real async cancellation channel.

### `generated_at_unix: 1` — `agent/mod.rs:74`
`probe_and_persist` hardcodes `generated_at_unix: 1` (epoch+1s). `ProductionRuntime` has a `Clock` trait with `now()`, but `probe_and_persist` doesn't accept a clock parameter.

### `ValidationCommandSource` enum — `types.rs:69-74`
All four variants (`CliOverride`, `ConfigValidation`, `StartupValidation`, `AutoDiscovery`) are constructed in `config.rs`, but `.source` is never pattern-matched or read in production code. Effectively write-only.

### `ValidationCommandResolution` startup fields — `types.rs:80-81`
`startup_validate_on_boot` and `startup_validation_command` are set in every branch of `resolve_validation_command`, but never read. `StartupSnapshot` only uses `.command`.

### `SchedulerMetrics` — `scheduler.rs:15,19`
`claim_latency_ms` stays `0`, `requeue_count` never incremented. Neither is read for any logic.

---

## 5. Config Fields Parsed But Never Used at Runtime

**`config.rs`** — these fields are deserialized from TOML and stored but consulted nowhere:

| Field | Struct | Line |
|---|---|---|
| `allow_agent_discovery` | `ValidationConfig` | ~63 |
| `heartbeat_interval_seconds` | `SchedulerConfig` | ~80 |
| `starvation_threshold_seconds` | `SchedulerConfig` | ~81 |
| `reconcile_interval_seconds` | `SchedulerConfig` | ~82 |
| `stale_after_days` | `QualityReportConfig` | ~130 |
| `stale_if_head_commit_differs` | `QualityReportConfig` | ~131 |

Note: `report_stamp_is_stale` in `startup.rs` uses the hardcoded constant `REPORT_TTL_SECONDS = 3600` instead of reading `stale_after_days`. The only `SchedulerConfig` field actually read is `lease_timeout_seconds`.

---

## 6. Modules/Functions Implemented But Not Wired to Production

### `WorktreeClient` — `worktree.rs`
All methods (`create_or_resume`, `remove_recreate_if_stale_empty`, `cleanup_on_completion`, `prune_orphans`) are implemented and tested, but the module is never imported by any production `src/` file.

### `GhClient` — `gh.rs`
The entire client plus `upgrade_unmerged_collision_priority` is implemented and tested internally, but `gh.rs` is not imported by any production `src/` file.

### `is_stale` — `repo_intelligence.rs:120`
Implemented but no production caller. The logic is duplicated directly inside `triage_needed` in `triage.rs`.

### `profile_path_from_config` — `repo_intelligence.rs:250-258`
Has `#[allow(dead_code)]`. Zero callers outside the file.

### `decay_confidence` — `prompt_knowledge.rs:14`
Only called in its own unit test. `LearningConfig.confidence_decay_per_day` exists in config but is never passed to this function at runtime.

### `profile_exists` — `triage.rs:157`
Only called from `tests/phase03_startup.rs:36`. Not called from any production code path.

### `restart_with_resume_link` — `worker_identity.rs:40`
Only called in its own unit test. Production retry path calls `begin_retry()` instead.

### `handle_key` — `tui.rs:378`
Only called in tests. The actual dispatch path uses `hotkeys::action_for_key` returning `HotkeyAction` enum values, not this function (which returns `&'static str`).

### `classify_priority` / `ClassifierInput` / `Priority::rank` — `priority.rs`
All public but only called in tests within `priority.rs`. Not used by any production code.

### `GitClient::clear_stale_lock_file` — `git.rs:24`
`GitClient` itself is only instantiated in `gh.rs` tests. This method has no production caller.

---

## Summary by Priority

**High (stubs with real scaffolding already in place):**
- `reconcile_worktrees()` — `WorktreeClient` exists and is tested, just not connected
- `reconcile_open_prs()` — `GhClient.upgrade_unmerged_collision_priority` exists, just not called
- `Retry` / `ReleaseLease` / `ParkEscalate` hotkeys — fully routed, handler bodies empty

**Medium (data/config defined but ignored):**
- Config fields: `heartbeat_interval_seconds`, `starvation_threshold_seconds`, `reconcile_interval_seconds`, `stale_after_days`, `stale_if_head_commit_differs`, `allow_agent_discovery`
- `AdapterContext::knowledge_refs` — field plumbed through, never consumed
- `AdapterContext::cancel_requested` — check exists, never set to `true` in production
- `TeardownReport` boolean fields — always `true`, no real teardown

**Low (dead code, duplicated logic, or cleanup candidates):**
- `is_stale` vs inline logic in `triage_needed` — deduplication opportunity
- `handle_key` in `tui.rs` — shadow function alongside real hotkey dispatch
- `priority.rs` public API — no production consumers
- `ValidationCommandSource` enum — write-only from production's perspective
- `generated_at_unix: 1` hardcoded — clock trait exists but not plumbed in
