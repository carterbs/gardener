# Plan: Direct Event Streaming for Worker Command Display

## Context

The worker TUI command stream has three bugs:
1. **First character cutoff**: A global scroll offset (`COMMAND_SCROLL_TICK`) monotonically increments and never resets, causing `command_stream_window` to always pin to the right edge. The instant a command stream exceeds display width by N chars, the first N chars are invisible.
2. **Updates stall**: Both commands AND state changes flow through the JSONL log file — `recent_worker_tool_commands` and `recent_worker_state_events` re-read the **entire file from disk** every 25ms. This gets slower as the log grows and only runs on channel timeout.
3. **Wrong order**: Commands display oldest-first; user wants newest on the left.

The agents already emit events via `on_event` callbacks, and workers already emit state changes via `emit_worker_activity_state_with`. Both paths write to the log file, then the pool re-reads it. We'll replace both hot-path log reads with direct channel messaging.

## Changes

### 1. Unified event channel — `worker_pool.rs`

Add a channel for all worker UI events (commands + state changes):

```rust
struct WorkerUiEvent {
    slot_idx: usize,
    kind: WorkerUiEventKind,
}
enum WorkerUiEventKind {
    Command(String),
    StateChange { state: String, task_id: String, details: String },
}
```

Create `mpsc::channel::<WorkerUiEvent>()` alongside the existing `PoolResultMessage` channel (~line 300).

### 2. Command extraction — reuse `extract_agent_command` from `agent_command.rs`

The existing `extract_agent_command` (agent_command.rs:3) already handles the full cascade for both codex and claude payloads (`/item/command`, `/item/command_line`, `/item/cmd`, `/command`, `/input/command`, etc. with recursive descent into `item`, `input`, `payload` nodes).

Add a thin wrapper in `worker_pool.rs` that calls it on the `AgentEvent`:

```rust
fn summarize_agent_event(event: &AgentEvent) -> Option<String> {
    match event.kind {
        AgentEventKind::ToolCall | AgentEventKind::ToolResult => {
            let label = event.payload.pointer("/name")
                .or(event.payload.pointer("/item/name"))
                .or(event.payload.pointer("/item/type"))
                .and_then(Value::as_str)
                .unwrap_or(&event.raw_type);
            let cmd = extract_agent_command(&event.payload);
            match cmd {
                Some(c) => Some(format!("{label}: {c}")),
                None => Some(label.to_string()),
            }
        }
        _ => None,
    }
}
```

### 3. State change sink via thread-local — `worker.rs`

To avoid modifying ~45 `emit_worker_activity_state` call sites, add a thread-local sink:

```rust
thread_local! {
    static STATE_SINK: RefCell<Option<Box<dyn Fn(&str, &str, &str)>>> = RefCell::new(None);
}

pub(crate) fn install_state_sink(sink: Box<dyn Fn(&str, &str, &str)>) {
    STATE_SINK.with(|cell| *cell.borrow_mut() = Some(sink));
}

pub(crate) fn clear_state_sink() {
    STATE_SINK.with(|cell| *cell.borrow_mut() = None);
}
```

In `emit_worker_activity_state_with` (~line 130), after the existing `append_run_log` call, add:

```rust
STATE_SINK.with(|cell| {
    if let Some(sink) = cell.borrow().as_ref() {
        let details_str = /* format details from the json Value */;
        sink(state.as_str(), task_id, &details_str);
    }
});
```

### 4. Wire senders into worker spawns — `worker_pool.rs`

For each doing-worker spawn (~lines 353, 689):

```rust
let worker_ui_tx = ui_tx.clone();
let state_ui_tx = ui_tx.clone();
let slot = idx;
scope_guard.spawn(move || {
    // Install state change sink for this thread
    install_state_sink(Box::new(move |state, task_id, details| {
        let _ = state_ui_tx.send(WorkerUiEvent {
            slot_idx: slot,
            kind: WorkerUiEventKind::StateChange {
                state: state.to_string(),
                task_id: task_id.to_string(),
                details: details.to_string(),
            },
        });
    }));
    // Agent event callback for commands
    let on_event = move |event: &AgentEvent| {
        if let Some(summary) = summarize_agent_event(event) {
            let _ = worker_ui_tx.send(WorkerUiEvent {
                slot_idx: slot,
                kind: WorkerUiEventKind::Command(summary),
            });
        }
    };
    let result = execute_task(..., Some(&on_event));
    clear_state_sink();
    let _ = tx.send(PoolResultMessage::DoingResult { ... });
});
```

Same pattern for merge worker spawn (~line 382), using `merge_row_idx`.

### 5. Add `on_event` parameter to worker entry points — `worker.rs`

- `execute_task` (line 170): add `on_event: Option<&dyn Fn(&AgentEvent)>`
- `execute_task_live` (line 207): same
- `execute_merge_phase` (line 827): same
- Replace all ~12 `on_event: None` in `AgentTurnInput` with the passed-through `on_event`
- `execute_task_simulated` path ignores the callback (not passed through)

### 6. Drain channel in main loop — `worker_pool.rs`

Add `drain_ui_events(rx, workers) -> bool` that calls `try_recv()` in a loop:
- `Command` → `append_worker_command(worker, &cmd)`
- `StateChange` → apply the same logic currently in `append_worker_state_events` (non-regressive transition check, update state/breadcrumb/tool_line)

Call this at the **top of every loop iteration**, before `match rx.recv_timeout(25ms)`.

### 7. Remove hot-path log-file polling — `worker_pool.rs`

Remove from the hot path:
- `append_worker_tool_commands` function (lines 1231-1254) and its call
- `append_worker_state_events` function (lines 1256-1301) and its call
- `last_worker_command_line` and `last_worker_state_line` variables
- `command_poll_chunk` variable
- Imports of `recent_worker_tool_commands` and `recent_worker_state_events`

Keep `recent_worker_tool_commands` and `recent_worker_state_events` in `logging.rs` — they're still used by tests and cold-path diagnostics (`recent_worker_log_lines` for failure prompts).

### 8. TUI: newest-first, no scroll window — `tui.rs`

**`worker_command_stream`** (line 1896): Remove the second `.rev()` so newest stays first:
```rust
fn worker_command_stream(commands: &[CommandEntry]) -> String {
    let recent: Vec<_> = commands.iter().rev().take(RECENT_COMMAND_STREAM_LIMIT).collect();
    if recent.is_empty() { return "no recent commands".to_string(); }
    recent.iter()
        .map(|entry| format!("{}  {}", entry.timestamp, entry.command))
        .collect::<Vec<_>>()
        .join("  |  ")
}
```

**`command_stream_window`** (line 1936): Replace sliding window with truncation using existing `truncate_right`:
```rust
fn command_stream_window(stream: &str, width: usize, _offset: usize) -> String {
    truncate_right(stream, width)
}
```

**Remove**: `current_command_scroll_offset()` (lines 1914-1934), `COMMAND_SCROLL_TICK` thread-local (line 1526), and the `command_scroll_offset` variable at call sites (line 1176).

### 9. Reduce history limit — `worker_pool.rs`

Change `WORKER_COMMAND_HISTORY_LIMIT` from 32 to 20 (line 35).

### 10. Update tests

- `worker.rs` test at line 1937: add `None` for new `on_event` param
- `worker.rs` test at line 2138: add `None` for new `on_event` param
- `tui.rs`: update tests for `worker_command_stream` (newest-first) and `command_stream_window` (truncation)
- Remove or update tests for deleted `append_worker_tool_commands`/`append_worker_state_events`

## Files Modified

| File | Changes |
|---|---|
| `tools/gardener/src/worker_pool.rs` | WorkerUiEvent, channel, drain, summarize_agent_event, wire spawns, remove both polling fns |
| `tools/gardener/src/worker.rs` | Thread-local state sink, on_event param on 3 functions, replace ~12 on_event: None |
| `tools/gardener/src/tui.rs` | Newest-first ordering, truncation instead of scroll window, remove COMMAND_SCROLL_TICK |

## Verification

1. `cargo build` — confirms compilation
2. `cargo test` — all tests pass
3. Manual: run gardener, verify newest commands appear on the left and first character is never cut off
4. Manual: verify commands and state changes update continuously without stalling
5. Manual: confirm the OTEL log file is still written (for diagnostics) but no longer read on the hot path
