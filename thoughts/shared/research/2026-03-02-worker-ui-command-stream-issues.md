# Worker UI Command Stream Issues

**Date**: 2026-03-02
**Status**: Research complete
**Reported symptoms**: (1) first character frequently cut off, (2) updates sometimes stop, (3) agent commands should flow to worker display

---

## Architecture Overview

The data flow from agent → TUI is **indirect** and goes through a log file:

```
Agent subprocess (claude CLI)
  → ClaudeAdapter.on_stdout_line (agent/claude.rs:128)
    → append_run_log("debug", "adapter.claude.event", {...})  (agent/claude.rs:138)
      → writes JSONL to run log file on disk
        ↓
Worker Pool main loop (worker_pool.rs:849, every 25ms timeout)
  → recent_worker_tool_commands(from_line, 32)  (logging.rs:338)
    → reads ENTIRE log file from disk
    → scans for event_type.starts_with("adapter.")
    → extract_payload_command() to get command string
    → returns Vec<(line_num, worker_id, command)>
  → append_worker_command(worker, command)  (worker_pool.rs:1185)
    → pushes (timestamp, command) to WorkerRow.command_details
      ↓
TUI render (tui.rs:1196)
  → worker_command_stream(commands)  (tui.rs:1896)
    → takes last 4 commands, joins chronologically with " | "
  → command_stream_window(stream, width, scroll_offset)  (tui.rs:1936)
    → sliding window into the flat string
```

**Critical**: `worker.rs` passes `on_event: None` to every `AgentTurnInput` (lines 269, 322, 365, 538, 575, 610, 1041, 1112, 1165, 1246, 1685). Events do NOT stream directly to the worker pool. They flow through the disk-based log file only.

---

## Issue 1: First Character Cut Off

### Root Cause: Global monotonic scroll offset + sliding window

`command_stream_window` (tui.rs:1936):
```rust
fn command_stream_window(stream: &str, width: usize, offset: usize) -> String {
    let chars: Vec<char> = stream.chars().collect();
    let len = chars.len();
    if len <= width {
        return stream.to_string();  // fits — no cutoff
    }
    let max_offset = len.saturating_sub(width);
    let start = offset.min(max_offset);  // ← HERE
    chars[start..start + width].iter().collect()
}
```

`current_command_scroll_offset` (tui.rs:1914):
```rust
// Global offset that increments every 120ms and NEVER resets
COMMAND_SCROLL_TICK: RefCell<(usize, u128)> = (0, 0);
// After 1 minute: offset = 500
// After 5 minutes: offset = 2500
// After 1 hour: offset = 30000
```

**What happens**: The global offset grows unbounded. When the command stream string is longer than the display width:

1. `max_offset = stream_len - display_width`
2. `start = huge_global_offset.min(max_offset) = max_offset`
3. Display ALWAYS shows the **rightmost** `width` characters

**Example**: Stream is `"01:23:45  git status  |  01:24:00  cargo build"` (47 chars), width is 45:
- `max_offset = 47 - 45 = 2`
- Display starts at char 2: `":23:45  git status  |  01:24:00  cargo build"`
- **First two characters ("01") are cut off**

This gets worse as more commands arrive. With 3-4 commands the stream easily reaches 100+ chars, and `max_offset` could be 40-60, meaning 40-60 leading characters are always invisible.

**The scroll offset never resets** — not when new commands arrive, not when workers change state, not when command_details is cleared. Once the gardener has been running for a few seconds, the offset is permanently large enough to always pin to the right end.

### Why "first character" specifically

The user likely notices this most when a new command just arrives and the stream crosses the `width` boundary. At that moment `max_offset` goes from 0 to 1, and the display instantly clips the first character (e.g., the leading digit of the timestamp).

---

## Issue 2: Updates Sometimes Stop

### Multiple contributing factors:

**A. Log file polling only happens on channel timeout** (worker_pool.rs:849-859)

```rust
match rx.recv_timeout(Duration::from_millis(25)) {
    Ok(PoolResultMessage::DoingResult { .. }) => {
        // processes result, renders, but does NOT poll tool commands
    }
    Err(RecvTimeoutError::Timeout) => {
        // ONLY HERE are tool commands polled
        append_worker_tool_commands(...);
        append_worker_state_events(...);
    }
}
```

When worker completion messages arrive on the channel, the timeout branch is skipped. During bursts of activity (workers completing, merges finishing), command polling stalls entirely.

**B. `set_worker_idle` clears all command history** (worker_pool.rs:1195-1206)

```rust
fn set_worker_idle(worker: &mut WorkerRow, tool_line: &str) {
    worker.command_details.clear();  // ← everything gone
}
```

Between phases (understand → planning → doing), the worker goes idle briefly. Its entire command history is wiped. When it starts the next phase, the display is blank until new events flow through the log→poll→render pipeline (which takes multiple 25ms cycles).

**C. Full log file re-read on every poll** (logging.rs:361)

```rust
let text = match std::fs::read_to_string(&path) {
    Ok(text) => text,
    Err(_) => return Vec::new(),  // silent failure
};
```

The entire JSONL file is read from disk on every 25ms poll. As the run progresses and the log grows to thousands of lines, this gets slower. Combined with the `run_log_activity_lock` mutex (shared with the adapter writer threads), contention can delay polling.

**D. Conditional render gate** (worker_pool.rs:860-862)

```rust
if updated_commands || updated_states
    || last_dashboard_refresh.elapsed() >= Duration::from_secs(1)
```

If no new commands AND no new state events AND less than 1 second has passed, no render happens. The command scroll animation (which should update the visible window every 120ms) only advances when a render actually occurs. So the display can appear frozen for up to 1 second between renders when nothing new is detected.

---

## Issue 3: Command Display Order

Currently `worker_command_stream` (tui.rs:1896) builds the stream chronologically (oldest first):
```rust
let recent = commands.iter().rev().take(4).collect::<Vec<_>>();
// reverse back to chronological order
recent.into_iter().rev().map(|entry| format!("{}  {}", entry.timestamp, entry.command))
    .join("  |  ")
```

With the scroll offset pinning to the right, the newest command IS at the rightmost position. But it's a flat string — there's no clear visual separation between "newest" and "older" commands. The user wants newest-first ordering so the most recent command is always the first thing visible.

---

## Key Files

| File | Lines | What |
|---|---|---|
| `tui.rs` | 1896-1944 | `worker_command_stream`, `command_stream_window`, `current_command_scroll_offset` |
| `tui.rs` | 1518-1526 | `COMMAND_SCROLL_TICK` thread-local (global, never-reset offset) |
| `tui.rs` | 1173-1201 | Command stream width calculation and render |
| `worker_pool.rs` | 849-859 | Log file polling (only on timeout) |
| `worker_pool.rs` | 1185-1193 | `append_worker_command` |
| `worker_pool.rs` | 1195-1206 | `set_worker_idle` (clears command_details) |
| `worker_pool.rs` | 1231-1254 | `append_worker_tool_commands` |
| `logging.rs` | 338-410 | `recent_worker_tool_commands` (full file read + scan) |
| `logging.rs` | 493-523 | `extract_payload_command` |
| `agent/claude.rs` | 128-151 | Where adapter events get logged |
| `worker.rs` | 269+ | All `on_event: None` calls |
