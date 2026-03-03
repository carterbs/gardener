---
date: 2026-03-03
researcher: codex
git_commit: 405f516fe01585bc25e204b22791e784005431fc
branch: main
topic: stale lease recovery call paths in lib.rs and worker_pool.rs
tags: [backlog, leases, worker-pool]
status: complete
---

# Research Question

What call paths invoke `recover_stale_leases` (or equivalent stale-lease recovery) from `tools/gardener/src/lib.rs` and `tools/gardener/src/worker_pool.rs`, when do they run, and which task statuses do they mutate?

# Summary

`recover_stale_leases` is invoked in three runtime paths: once in `--sync-only`, once during normal startup before dispatching the worker pool, and on two operator-only TUI hotkeys (`Retry`, `ReleaseLease`). There is no periodic/automatic stale-lease recovery loop in `run_worker_pool_fsm`; recovery is event-driven at startup and operator action.

The underlying SQL implementation (`recover_stale`) is broader than just expired leased rows: it also rewrites any `in_progress` and `merge_pending` tasks regardless of lease expiry. Rows with `related_pr IS NULL` become `ready`; rows with `related_pr IS NOT NULL` become `merge_pending`. In both branches, lease metadata is cleared and `last_updated` is advanced.

# Detailed Findings

## Call Paths and Runtime Timing

- Entrypoint: `run()` delegates to `run_with_runtime()`.
- `--sync-only` path executes `store.recover_stale_leases(system_time_unix())` immediately after opening the DB and before snapshot export.
- Worker path executes `store.recover_stale_leases(system_time_unix())` after startup audits/seeding and before backlog snapshot/logging and `run_worker_pool_fsm(...)` dispatch.
- Inside the worker pool loop, `handle_hotkeys(...)` is polled every iteration. Recovery happens only when operator hotkeys are enabled and user presses:
  - `r` (`Retry`): `recover_stale_leases(now_unix_millis())`
  - `l` (`ReleaseLease`): `recover_stale_leases(now + lease_timeout + 1s)` to force-release all `leased` rows as stale.

## Status Effects

Underlying SQL (`recover_stale`) applies two updates:

1. `related_pr IS NULL` rows:
- Source statuses: `in_progress`, `merge_pending`, or `leased` with missing/expired lease.
- Target status: `ready`.

2. `related_pr IS NOT NULL` rows:
- Source statuses: same set (`in_progress`, `merge_pending`, or stale `leased`).
- Target status: `merge_pending`.

Common side effects for both updates:
- `lease_owner = NULL`
- `lease_expires_at = NULL`
- `last_updated = now`

Practical implication:
- Startup recovery can unblock previously stuck tasks before claiming begins.
- Hotkey `l` is effectively a forced global lease clear by advancing `now` beyond timeout.
- Because `in_progress` and `merge_pending` are included unconditionally, manual recovery can re-queue active tasks if invoked at the wrong time.

# Code References

| File | Lines | Description |
|------|-------|-------------|
| `tools/gardener/src/lib.rs` | 159-175 | Runtime entrypoint to `run_with_runtime`. |
| `tools/gardener/src/lib.rs` | 364-393 | `--sync-only` branch; stale lease recovery before snapshot export. |
| `tools/gardener/src/lib.rs` | 516 | Startup stale lease recovery before worker dispatch. |
| `tools/gardener/src/lib.rs` | 589-597 | Worker pool start after startup reconciliation. |
| `tools/gardener/src/worker_pool.rs` | 255-267 | `handle_hotkeys` called each worker-loop iteration. |
| `tools/gardener/src/worker_pool.rs` | 1306-1308 | Hotkeys disabled when stdin is not a TTY. |
| `tools/gardener/src/worker_pool.rs` | 1343-1345 | `Retry` hotkey recovery call with current time. |
| `tools/gardener/src/worker_pool.rs` | 1358-1362 | `ReleaseLease` hotkey force-recovery call using `now + timeout + 1s`. |
| `tools/gardener/src/worker_pool.rs` | 1438-1443 | `now_unix_millis()` timebase used by hotkeys. |
| `tools/gardener/src/backlog_store.rs` | 1132-1167 | `recover_stale_leases` API and logging behavior. |
| `tools/gardener/src/backlog_store.rs` | 1973-2007 | `recover_stale` SQL status transitions and lease clearing. |

# Architecture Insights

Stale lease recovery is centralized in `BacklogStore` write-thread command handling (`WriteCmd::RecoverStale`), keeping all task-state mutation in serialized DB writes. `lib.rs` uses this as startup hygiene, while `worker_pool.rs` exposes operator recovery controls via hotkeys.

# Historical Context

No historical analysis was required for this question.

# Open Questions

- Should `recover_stale` touch `in_progress`/`merge_pending` unconditionally, or only when lease has actually expired?
- Should there be a periodic safe reclaim mechanism rather than operator-triggered recovery only?
