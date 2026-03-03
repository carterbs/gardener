---
date: 2026-03-03
researcher: codex
git_commit: 405f516fe01585bc25e204b22791e784005431fc
branch: main
topic: Whether recover_stale_leases clears lease_owner on complete backlog tasks
tags: [backlog, leases, runtime]
status: complete
---

# Research Question

Does the backlog stale-lease recovery path (`recover_stale_leases`) clear lingering `lease_owner` values for rows already in `status = 'complete'`?

# Summary

No. The stale-lease recovery SQL does not include `complete` in its status predicate, so already-complete rows are not touched by this path.

`recover_stale` only updates rows in `in_progress`, `merge_pending`, or stale `leased` states. For rows it does match, it nulls `lease_owner`/`lease_expires_at` and sets status to `ready` (no related PR) or `merge_pending` (with related PR).

# Detailed Findings

## Stale lease recovery predicate

`recover_stale_leases` delegates to `recover_stale`, which runs two UPDATE statements constrained to non-complete active states. Both statements clear lease metadata but only for matched statuses.

## Where recovery is invoked

Recovery runs during startup flows and via operator hotkeys (`r` retry, `l` release lease) in worker-pool TTY mode. Invocation frequency affects when cleanup happens, but not what statuses are eligible.

# Code References

| File | Lines | Description |
|------|-------|-------------|
| `tools/gardener/src/backlog_store.rs` | 1973-2003 | `recover_stale` SQL predicates and lease clearing behavior |
| `tools/gardener/src/backlog_store.rs` | 1767-1769 | `mark_complete` transition clears lease fields when moving into `complete` |
| `tools/gardener/src/backlog_store.rs` | 1619-1631 | `upsert_task` conflict update normalizes lease fields for non-leased/in-progress statuses |
| `tools/gardener/src/lib.rs` | 387, 516 | startup call sites for `recover_stale_leases` |
| `tools/gardener/src/worker_pool.rs` | 1344, 1361 | hotkey-triggered recovery paths |

# Architecture Insights

Lease cleanup is primarily modeled as part of state transitions (`mark_complete`, `release_lease`, `mark_unresolved`) and stale-active recovery (`recover_stale`). It is not a blanket invariant sweeper for terminal states like `complete`.

# Historical Context

Not investigated in this pass.

# Open Questions

- Should a maintenance migration or periodic invariant repair clear `lease_owner` for all statuses outside `leased`/`in_progress`?
