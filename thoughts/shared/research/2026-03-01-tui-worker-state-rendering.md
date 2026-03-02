# TUI Worker State Rendering — End to End

**Date:** 2026-03-01
**Purpose:** Understand the full rendering pipeline for worker state in the dashboard, and identify opportunities for improvement.

---

## Data Flow

```
OTEL log file (otel-logs.jsonl)
  ↓ polled every 25ms (recv_timeout)
append_worker_tool_commands()   ← adapter.* events → command_details[]
append_worker_state_events()    ← worker.activity.state_changed → state, tool_line, breadcrumb

worker_pool FSM (on task lifecycle)
  → directly writes to WorkerRow: state, task_title, tool_line, breadcrumb, command_details

WorkerRow[] ──→ render() ──→ draw_dashboard_frame()
                                ↓
                         AppState::from_dashboard_feed()
                                ↓
                         WorkerCard[] ──→ List<ListItem>
```

---

## WorkerRow Fields

| Field | Source | Used In |
|---|---|---|
| `worker_id` | FSM init | header label |
| `state` | FSM + state events | flow chain, state_bucket |
| `task_title` | FSM on claim | line 1 |
| `tool_line` | FSM + state events | (currently unused in worker list render) |
| `breadcrumb` | FSM + state events | (unused in worker list render) |
| `last_heartbeat_secs` | refresh_worker_heartbeats | (not shown for doing workers) |
| `session_age_secs` | refresh_worker_heartbeats | (not shown) |
| `lease_held` | FSM | (not shown) |
| `session_missing` | FSM | (not shown) |
| `command_details` | append_worker_command() | Commands line |

---

## Worker Card Render (3 lines, non-compact)

```
> Lawn Mower: Fix authentication bug in login flow
      Flow:  Understand → Planning → [Doing] → Gitting → Reviewing → Merging → Complete
      Commands: 14:32:01 claimed  |  14:32:02 state doing  |  14:33:10 state commit  |  14:33:22 Bash: cargo test
```

- **Line 1**: `{marker} {name}: {task_title}` — selection marker, equipment name, task
- **Line 2**: Flow chain — coarse state highlighted green; past states gray; future dimmed
- **Line 3**: Command stream — last 4 commands joined `  |  `, horizontally auto-scrolled at 120ms/char

**Compact mode** (width ≤ 80): Lines 1+2 only (no Commands line).

---

## Command Stream Mechanics

In `append_worker_command()` (worker_pool.rs:1127):
```rust
worker.command_details.push((timestamp, command));  // append, newest last
if len > 32 { drain oldest }
```

In `worker_command_stream()` (tui.rs:1900):
```rust
commands.iter().rev().take(4)  // take last 4 newest
  .rev()                        // reverse back to chronological
  .map(|e| format!("HH:MM:SS  cmd")).join("  |  ")
// Result: "oldest  |  ...  |  newest"
```

Then `command_stream_window()` auto-scrolls the string left-to-right at 120ms per character.
→ **The newest command ends up at the right end of a scrolling ticker.**

---

## State Mapping — Coarse vs Fine-Grained

`types.rs` defines `WorkerActivityState` with 22 fine-grained states:
- `GittingRemediation`, `PrCreating` (all mapped to "gitting" in flow bar)
- `MergePolling`, `MergeFromMain`, `MergeRemediation`, `CiFailureRemediation`, `PostMergeValidation` (all mapped to "merging")
- `WorktreePreparing`, `WorktreeReady` (mapped to "understand")

`normalize_worker_state()` (tui.rs:1951) collapses everything to 7 flow states.
→ **A worker stuck in CiFailureRemediation looks identical to one actively merging.**

The `worker_state_details()` function in logging.rs DOES extract rich detail for merge states (`attempt`, `pr_number`, `merge_state_status`, `next_check_in_secs`) — but this only surfaces in the merge worker card's `tool_line`, never in the doing-worker list.

---

## Fields Tracked But Not Displayed

- `tool_line` — the "Action" field — is only shown in the **merge worker** card, not in the doing-worker list render
- `breadcrumb` — only shown in activity entry generation, not directly rendered
- `last_heartbeat_secs` / `session_age_secs` — computed but never shown for doing workers
- `lease_held` — tracked, never shown
- `session_missing` — tracked, never shown

---

## Suggestions

### 1. Prepend commands (user-identified)
**Problem:** Newest command is rightmost in a scrolling ticker. By the time you look at a worker, the scroll has advanced past the relevant part.
**Fix:** Reverse the display order — newest first. This eliminates the need for horizontal scrolling. The most recent action is always at position 0 (left edge).

```
Commands: 14:33:22 Bash: cargo test  |  14:33:10 state commit  |  14:32:02 state doing
```

Change `worker_command_stream()`: remove the second `.rev()` and don't scroll.

---

### 2. Show `tool_line` in the doing-worker list
**Problem:** `tool_line` contains the most specific description of what a worker is doing right now (e.g., `"Checking mergeability (attempt=2, pr_number=53, merge_state_status=DIRTY, next_check_at=14:45:10)"`), but it's only rendered in the **merge worker** card.
**Fix:** Replace or supplement the flow chain line with `tool_line` for doing workers. The flow chain is good for orientation but `tool_line` has the actual actionable context.

Candidate layout:
```
> Lawn Mower: Fix authentication bug
      Flow:  Understand → [Doing] → Gitting → Reviewing → Merging → Complete
      Action: Doing (attempt=1)   Commands: 14:33:22 Bash: cargo test  |  ...
```

---

### 3. Show time-in-current-state
**Problem:** `last_heartbeat_secs` is tracked but never shown for doing workers. A worker that's been in `merge_polling` for 45 minutes looks identical to one that just entered it.
**Fix:** Add a heartbeat/age indicator to line 1 or the flow line. Even a simple `[2m]` suffix on the highlighted state step would help spot stalled workers.

```
Flow:  Understand → [Doing  3m] → Gitting → Reviewing → Merging → Complete
```

---

### 4. Surface fine-grained merge sub-states in the flow bar
**Problem:** `MergePolling`, `CiFailureRemediation`, `MergeRemediation` all collapse to "Merging" in the flow bar. When the merge loop retries for 20 minutes there's no visual signal of *which* sub-state it's stuck in.
**Fix:** When the current state is a merge substate, show the substate label instead of "Merging":

```
Flow:  ... → Reviewing → [Checking mergeability] → Complete
```

`format_state_label()` already has the right strings ("Checking mergeability", "Merge Remediation", etc.) — just needs `normalize_worker_state()` to not collapse them for display purposes.

---

### 5. Dim idle workers
**Problem:** Workers waiting for work take up equal visual weight as active workers.
**Fix:** Apply `DIM` modifier to the entire row for idle workers (state == "idle"), similar to how future flow states are styled. Makes it easy to visually skip past idle slots.

---

### 6. Show `session_age_secs` in worker header
**Problem:** No indication of total session duration for a worker.
**Fix:** Append elapsed time to the worker name line, formatted as `[HH:MM]`:

```
> Lawn Mower  [1:23]: Fix authentication bug
```

---

## Priority Ranking

| # | Suggestion | Effort | Impact |
|---|---|---|---|
| 1 | Prepend commands (newest-first, no scroll) | Low | High — immediately shows current activity |
| 2 | Show `tool_line` in doing-worker list | Low | High — rich context already computed |
| 3 | Dim idle workers | Low | Medium — cleaner visual hierarchy |
| 4 | Time-in-state on flow bar | Medium | Medium — spots stalled workers |
| 5 | Fine-grained merge substates in flow | Medium | Medium — merge loop visibility |
| 6 | Session age in header | Low | Low — nice to have |
