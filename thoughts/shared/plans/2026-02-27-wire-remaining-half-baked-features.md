# Wire Remaining Half-Baked Features

**Date:** 2026-02-27
**Scope:** `tools/gardener/src/`
**Input:** `thoughts/shared/research/2026-02-27-half-implemented-unwired-features.md`

---

## Overview

The research doc cataloged ~30 unwired items. Commit `3b4ccd3` already wired the reconcile stubs (`reconcile_worktrees`, `reconcile_open_prs`) and hotkey handlers (`Retry`, `ReleaseLease`, `ParkEscalate`). This plan covers the **9 items still broken** plus dead code cleanup.

## What's Left (Verified Against Current Code)

| # | Item | Category |
|---|------|----------|
| 1 | `collect_evidence()` hardcodes `src/lib.rs` / `src/main.rs` | Stub |
| 2 | `teardown_after_completion` ignores worktree client/path | Unwired |
| 3 | `validate_on_boot` failure logs but doesn't enqueue P0 | Unwired |
| 4 | `agents_md_present` hardcoded `false` in `build_profile` | Unwired |
| 5 | `generated_at_unix` bypasses `Clock` trait | Unwired |
| 6 | TUI hardcodes `15`/`900` instead of config values | Unwired |
| 7 | `stale_if_head_commit_differs` config never read | Unwired |
| 8 | `UserValidated` correction fields always empty | Deferred |
| 9 | `knowledge_refs` / `cancel_requested` always default | Deferred |

**Deferred rationale:** Items 8-9 need new features (interview questions, signal mechanism) that don't exist yet. Wiring empty data to empty consumers accomplishes nothing. We'll remove the dead plumbing instead.

## What We're NOT Doing

- Adding new interview questions for `UserValidated` correction fields
- Building a cancellation signal mechanism for `cancel_requested`
- Populating `knowledge_refs` (no source of knowledge refs exists)
- Changing `upgrade_unmerged_collision_priority` (always-P0 is correct by contract)
- Touching the `SchedulerEngine::run_stub_complete` seeding (only fires when store is empty)

---

## Phase 1: Wire Existing Pieces to Their Consumers

These are all cases where real implementations exist but aren't connected.

### 1a. `teardown_after_completion` — use the worktree client

**File:** `worker.rs:588-600`

**Change:** Remove `_` prefixes from `worktree_client` and `worktree_path`. Call `worktree_client.cleanup_on_completion(worktree_path)` and set `worktree_cleaned` based on the result.

```rust
fn teardown_after_completion(
    worktree_client: &WorktreeClient<'_>,
    worktree_path: &Path,
    output: &MergingOutput,
) -> TeardownReport {
    let worktree_cleaned = if output.merged {
        worktree_client.cleanup_on_completion(worktree_path).is_ok()
    } else {
        false
    };
    TeardownReport {
        merge_verified: output.merged,
        session_torn_down: output.merged,
        sandbox_torn_down: output.merged,
        worktree_cleaned,
        state_cleared: output.merged,
    }
}
```

### 1b. `validate_on_boot` — enqueue the P0 recovery task

**File:** `startup.rs:111-115`

**Change:** When `exit_code != 0`, open the `BacklogStore` and create a P0 Maintenance task. The store open pattern already exists nearby in `run_startup`.

```rust
if out.exit_code != 0 {
    runtime.terminal.write_line(
        "WARN startup validation failed; enqueueing P0 recovery task"
    )?;
    let db_path = scope.repo_root.as_ref()
        .unwrap_or(&scope.working_dir)
        .join(".cache/gardener/backlog.sqlite");
    let store = BacklogStore::open(db_path)?;
    store.upsert_task(NewTask {
        kind: TaskKind::Maintenance,
        title: "Recovery: startup validation failed".to_string(),
        details: format!("exit code {}", out.exit_code),
        scope_key: "startup".to_string(),
        priority: Priority::P0,
        source: "validate_on_boot".to_string(),
        related_pr: None,
        related_branch: None,
    })?;
}
```

### 1c. `agents_md_present` — forward detection into profile

**File:** `repo_intelligence.rs` (`build_profile`) and `triage.rs` (call site)

**Change:** Add `agents_md_present: bool` parameter to `build_profile`. Pass `detected.agents_md_present` from `triage.rs`.

### 1d. `generated_at_unix` — plumb Clock into `probe_and_persist`

**File:** `agent/mod.rs:63-77`

**Change:** Add `clock: &dyn Clock` parameter. Replace `SystemTime::now()` with `clock.now()`. Update call site in `lib.rs` to pass `runtime.clock.as_ref()`.

### 1e. TUI config values — thread heartbeat/lease through

**File:** `tui.rs:234` and its call chain

**Change:** Add `heartbeat_interval_seconds: u64` and `lease_timeout_seconds: u64` parameters to `draw_dashboard_frame`. Thread from `AppConfig` through `draw_dashboard_live` → `draw_dashboard_frame`. Replace hardcoded `15` and `900`.

### 1f. `stale_if_head_commit_differs` — wire into report staleness check

**File:** `startup.rs` (`report_stamp_is_stale`)

**Change:** When `cfg.quality_report.stale_if_head_commit_differs` is `true`, also check if the current head SHA differs from what was recorded. This requires adding a `process_runner` + `cwd` parameter (or head SHA) to the staleness check. The `current_head_sha` function already exists in `repo_intelligence.rs`.

**Success criteria:**
- `cargo build` passes
- `cargo test` passes
- Each wired function uses its parameters (no `_` prefixes on used args)

---

## Phase 2: Real Evidence Collection

### 2a. `collect_evidence()` — scan actual filesystem

**File:** `quality_evidence.rs:10-19`

**Change:** Replace hardcoded file lists with actual filesystem inspection. For each `QualityDomain`, walk the source tree and classify files as tested/untested based on whether a corresponding test file exists (e.g., `foo.rs` → `foo_test.rs` or `tests/foo.rs`).

The function needs a `FileSystem` or `ProcessRunner` parameter (or just `Path` for the repo root) to do real filesystem inspection. The approach:
1. Add `repo_root: &Path` parameter
2. For each domain, glob `src/**/*.rs` files
3. For each source file, check if a test counterpart exists (same module with `#[cfg(test)]` or a file in `tests/`)
4. A simpler approach: use `grep -l "#\[test\]"` to find files containing tests

Update `render_quality_grade_document` call site to pass repo root.

### 2b. `discover_domains()` — detect real domains

**File:** `quality_domain_catalog.rs:6-10`

Currently returns `[QualityDomain { name: "core" }]`. Could scan for top-level modules or directory structure. But this is lower priority — having one "core" domain with real file data is better than multiple fake domains.

**Decision:** Keep single "core" domain for now, just fix evidence collection.

**Success criteria:**
- `collect_evidence` returns real file paths from the filesystem
- Quality grades reflect actual test coverage state
- `cargo test` passes

---

## Phase 3: Clean Up Dead Code & Unused Plumbing

### 3a. Remove dead functions

| Function | File | Reason |
|----------|------|--------|
| `is_stale` | `repo_intelligence.rs:120` | Logic duplicated inline in `triage.rs` |
| `handle_key` | `tui.rs:378` | Shadow of real hotkey dispatch, test-only |
| `classify_priority` + `ClassifierInput` | `priority.rs` | Test-only, no production consumers |
| `Priority::rank` | `priority.rs` | Test-only |
| `AgentKind::parse_cli` | `types.rs:12-18` | Zero callers |
| `profile_path_from_config` | `repo_intelligence.rs:250` | `#[allow(dead_code)]`, duplicated |
| `_is_unknown` | `repo_intelligence.rs:260` | `#[allow(dead_code)]`, zero callers |

### 3b. Remove write-only fields and dead enum variants

| Item | File | Action |
|------|------|--------|
| `ValidationCommandSource` enum + `.source` field | `types.rs`, `config.rs` | Remove enum and field from `ValidationCommandResolution` |
| `WorkerState::Seeding` | `types.rs:45` | Remove variant + exhaustive match arms + `let _ =` in seed_runner |
| `UserValidated` correction fields | `repo_intelligence.rs` | Remove `agent_steering_correction`, `external_docs_surface`, `guardrails_correction`, `coverage_grade_override` — they have no source |
| `AdapterContext::knowledge_refs` | `agent/mod.rs` | Remove field and all `vec![]` constructions |
| `OpenPr::number` | `pr_audit.rs` | Remove field (only `head_ref_name` is used) |

### 3c. Remove unused config fields

| Field | Action |
|-------|--------|
| `allow_agent_discovery` | Remove from `ValidationConfig` — no consumer and no gating logic exists |
| `starvation_threshold_seconds` | Remove from `SchedulerConfig` — no consumer |
| `reconcile_interval_seconds` | Remove from `SchedulerConfig` — no consumer |

Keep `heartbeat_interval_seconds` (wired in Phase 1e).
Keep `stale_after_days` (already wired in `startup.rs`).
Keep `stale_if_head_commit_differs` (wired in Phase 1f).

### 3d. Clean up `cancel_requested`

Remove from `AdapterContext`. The cancel check in `claude.rs:78-83` and `codex.rs:88-93` can be removed too — there's no signal mechanism and building one is out of scope. When cancellation is actually needed, it'll be designed properly.

**Success criteria:**
- `cargo build` passes with no `#[allow(dead_code)]` attributes remaining on removed items
- `cargo test` passes (remove/update tests that reference deleted items)
- No new compiler warnings

---

## Phase 4: Verification

- `cargo build` — clean compile, no warnings
- `cargo test` — all tests pass
- `cargo clippy` — no new lints
- Manual review: grep for `_` prefixed parameters that should now be used
- Manual review: grep for remaining `#[allow(dead_code)]`

---

## References

- Research: `thoughts/shared/research/2026-02-27-half-implemented-unwired-features.md`
- Previous fix: commit `3b4ccd3` (wired reconcile stubs + hotkeys)
- Key files: `worker.rs`, `startup.rs`, `repo_intelligence.rs`, `triage.rs`, `agent/mod.rs`, `tui.rs`, `quality_evidence.rs`, `config.rs`, `types.rs`, `priority.rs`
