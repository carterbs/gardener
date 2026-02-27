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
Always returns `{stale_found: 0, stale_fixed: 0}`. Called in `startup.rs:97`. The real implementation (`WorktreeClient` in `worktree.rs`) is fully built and tested — it's just never imported by production code.

### `reconcile_open_prs()` — `pr_audit.rs:7-9`
```rust
pub fn reconcile_open_prs() -> PrAuditSummary {
    PrAuditSummary::default()
}
```
Always returns zeros. Called in `startup.rs:98`. `GhClient.upgrade_unmerged_collision_priority` in `gh.rs` exists and is tested, but `reconcile_open_prs()` never calls it.

### `collect_evidence()` — `quality_evidence.rs:10-19`
```rust
pub fn collect_evidence(domains: &[QualityDomain]) -> Vec<DomainEvidence> {
    domains.iter().map(|d| DomainEvidence {
        domain: d.name.clone(),
        tested_files: vec!["src/lib.rs".to_string()],    // hardcoded
        untested_files: vec!["src/main.rs".to_string()], // hardcoded
    }).collect()
}
```
Every domain gets the same two hardcoded filenames. No actual filesystem or coverage inspection happens. This feeds `score_domains` and then `render_quality_grade_document`, so the quality grades report is built on synthetic data.

### `upgrade_unmerged_collision_priority()` — `gh.rs:90-92`
```rust
pub fn upgrade_unmerged_collision_priority(_existing: Priority) -> Priority {
    Priority::P0
}
```
Parameter ignored, always returns `P0`. Not called from production code anyway — `reconcile_open_prs()` is the stub that would call it.

### `SchedulerEngine::run_stub_complete` — `scheduler.rs:60-102`
Self-named stub. Seeds placeholder tasks with hardcoded `"phase4"` / `"phase4-stub"` strings when the task store is empty.

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
Keys `r`, `l`, `p` are in `DASHBOARD_BINDINGS`, lint-tested, have enum variants, route correctly — handler bodies just print a string. No backlog mutation, lease release, or escalation logic exists.

---

## 3. Functions That Receive Parameters But Ignore Them

### `teardown_after_completion` — `worker.rs:588-600`
```rust
fn teardown_after_completion(
    _worktree_client: &WorktreeClient<'_>,  // ignored
    _worktree_path: &Path,                  // ignored
    output: &MergingOutput,
) -> TeardownReport {
    TeardownReport {
        merge_verified: output.merged,
        session_torn_down: output.merged,
        sandbox_torn_down: output.merged,
        worktree_cleaned: false,   // hardcoded
        state_cleared: output.merged,
    }
}
```
Both injected clients are prefixed `_` and unused. `worktree_cleaned` is hardcoded `false`. No actual teardown happens. The simulated path bypasses this function entirely and hardcodes all fields `true`.

### `upgrade_unmerged_collision_priority` — `gh.rs:90`
See above — `_existing` ignored, always returns `P0`.

---

## 4. Code Paths That Log Intent But Don't Act

### `validate_on_boot` failure branch — `startup.rs:100-116`
```rust
if out.exit_code != 0 {
    runtime.terminal.write_line(
        "WARN startup validation failed; enqueue P0 recovery task"
    )?;
    // <-- no task is actually enqueued
}
```
The message says "enqueue P0 recovery task" but nothing is enqueued. Execution continues normally.

---

## 5. Data Computed or Set But Immediately Discarded

### `doing_payload` discarded — `worker.rs` (in `execute_task_simulated`)
```rust
let doing_payload = DoingOutput { summary: "implementation complete".to_string(), ... };
// ...
let _ = doing_payload;
```

### `OpenPr::number` discarded — `pr_audit.rs:42`
```rust
let _ = pr.number;
```
Deserialized from JSON, immediately thrown away. Only `head_ref_name` is used.

### `let _ = WorkerState::Seeding` — `seed_runner.rs:55`
Bare discard to suppress compiler warnings. `Seeding` never appears as an FSM transition target.

### `_parsed` discarded — `worker.rs` (in `prepare_prompt`)
Parse result from a synthetic envelope immediately prefixed `_`. Validates the parser, has no effect on the returned `PreparedPrompt`.

---

## 6. Fields Always Hardcoded / Never Populated

### `agents_md_present` — `repo_intelligence.rs:178`
```rust
detected_agent: DetectedAgentProfile {
    agents_md_present: false,  // always hardcoded
    ...
}
```
`AgentDetection` in `triage_agent_detection.rs:38` actually detects `AGENTS.md` and sets this field — but the detected value is never forwarded into `DetectedAgentProfile` in `build_profile`.

### `UserValidated` correction fields — `repo_intelligence.rs:182-190`
Four fields always initialized to `String::new()` and never populated:
- `agent_steering_correction`
- `external_docs_surface`
- `guardrails_correction`
- `coverage_grade_override`

The triage interview only collects `validation_command`, `additional_context`, `external_docs_accessible`, and `preferred_parallelism`. None of these correction fields are ever set.

### `generated_at_unix: 1` — `agent/mod.rs:74`
`probe_and_persist` hardcodes epoch+1s. `ProductionRuntime` has a `Clock` trait with `now()`, but `probe_and_persist` doesn't accept a clock parameter.

### `AdapterContext::knowledge_refs` — `agent/mod.rs:21`
Every call site passes `vec![]`. Neither adapter reads the field. Never forwarded to the underlying CLI invocations.

### `AdapterContext::cancel_requested` — `claude.rs:78-83`, `codex.rs:88-93`
Cancel check exists post-spawn in both adapters, but every production call site hardcodes `false`. No mechanism to set it `true` during a live run.

---

## 7. Config Fields Parsed But Never Used at Runtime

**`config.rs`** — these fields are deserialized from TOML and stored but consulted nowhere:

| Field | Struct | Note |
|---|---|---|
| `allow_agent_discovery` | `ValidationConfig:63` | Parsed, stored, never branched on |
| `heartbeat_interval_seconds` | `SchedulerConfig:80` | Never read at runtime |
| `starvation_threshold_seconds` | `SchedulerConfig:81` | `tui.rs:234` hardcodes `900` instead |
| `reconcile_interval_seconds` | `SchedulerConfig:82` | Never read at runtime |
| `stale_after_days` | `QualityReportConfig:~130` | `startup.rs` uses hardcoded `REPORT_TTL_SECONDS = 3600` |
| `stale_if_head_commit_differs` | `QualityReportConfig:131` | Never read in any staleness check |

---

## 8. Modules/Functions Implemented But Not Wired to Production

### `WorktreeClient` — `worktree.rs`
All methods fully implemented and tested, module never imported by any production `src/` file.

### `GhClient` — `gh.rs`
Entire client implemented and tested internally, not imported by any production `src/` file.

### `is_stale` — `repo_intelligence.rs:120`
No production callers. Logic duplicated inline inside `triage_needed` in `triage.rs`.

### `profile_path_from_config` — `repo_intelligence.rs:250-258`
`#[allow(dead_code)]`, zero callers.

### `_is_unknown` — `repo_intelligence.rs:260-263`
`#[allow(dead_code)]`, named with `_` prefix, never called.

### `decay_confidence` — `prompt_knowledge.rs:14`
Only called in its own unit test. `LearningConfig.confidence_decay_per_day` exists in config but is never passed here at runtime.

### `profile_exists` — `triage.rs:157`
Only called from `tests/phase03_startup.rs:36`. Not called from production code.

### `restart_with_resume_link` — `worker_identity.rs:40`
Only called in its own unit test. Production retry uses `begin_retry()` instead.

### `handle_key` — `tui.rs:378`
Only called in tests. Runtime dispatch uses `hotkeys::action_for_key` (returns `HotkeyAction` enum) instead.

### `classify_priority` / `ClassifierInput` / `Priority::rank` — `priority.rs`
All public, all test-only. No production consumers.

### `GitClient::clear_stale_lock_file` — `git.rs:24`
No production caller. `GitClient` itself only instantiated in tests.

### `AgentKind::parse_cli` — `types.rs:12-18`
Only called from integration tests. Runtime uses `clap`'s `ValueEnum` + `Into<AgentKind>` instead.

---

## 9. Enum Variants / States Never Reached in Production

### `WorkerState::Seeding` — `types.rs:45`
Defined in the enum, falls through to `doing` budget in `token_budget_for_state`, but no `fsm.transition(WorkerState::Seeding)` call exists anywhere in production code.

### `ValidationCommandSource` — `types.rs:69-74`
All four variants constructed in `config.rs`, but `.source` is never pattern-matched or read in production code. Effectively write-only.

---

## Summary by Priority

**High (scaffolding ready, just not connected):**
- `reconcile_worktrees()` — `WorktreeClient` exists and tested, not connected
- `reconcile_open_prs()` — `GhClient` + `upgrade_unmerged_collision_priority` exist, not connected
- `collect_evidence()` — real quality grades built on hardcoded `src/lib.rs` / `src/main.rs`
- `Retry` / `ReleaseLease` / `ParkEscalate` hotkeys — fully routed, handler bodies empty
- `validate_on_boot` failure — logs "enqueue P0" but doesn't enqueue
- `agents_md_present` — detection logic exists, never forwarded into profile
- `UserValidated` correction fields — interview exists, fields never populated

**Medium (data/config defined but ignored):**
- Config fields: `heartbeat_interval_seconds`, `starvation_threshold_seconds`, `reconcile_interval_seconds`, `stale_after_days`, `stale_if_head_commit_differs`, `allow_agent_discovery`
- `AdapterContext::knowledge_refs` — plumbed through, never consumed
- `AdapterContext::cancel_requested` — check exists, never set `true` in production
- `teardown_after_completion` — receives clients, ignores them; no real teardown
- `generated_at_unix: 1` — clock trait exists but not plumbed in

**Low (dead code, duplication, cleanup candidates):**
- `is_stale` vs inline logic in `triage_needed`
- `handle_key` in `tui.rs` — shadow of real hotkey dispatch
- `priority.rs` public API — no production consumers
- `ValidationCommandSource` enum — write-only from production
- `AgentKind::parse_cli` — integration test relic
- `OpenPr::number` deserialized then immediately discarded
- `WorkerState::Seeding` — FSM sink state with dead `let _` reference
