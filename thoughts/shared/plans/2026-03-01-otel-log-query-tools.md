# OTEL Log Query Tools

**Date:** 2026-03-01
**Status:** Draft

---

## Overview

Add a Rust binary (`otel-logs`) with three subcommands that let agents query rotated OTEL log files deterministically. Agents currently grep raw JSONL files, which eats context and breaks silently when a run spans multiple rotated files. These tools provide structured, filtered, context-safe output.

---

## Current State Analysis

### Log format (`tools/gardener/src/logging.rs:196-229`)

Each JSONL line is a JSON object with:
- `event_type` — top-level string (e.g. `"adapter.claude.event"`, `"worker.activity.state_changed"`)
- `logRecord.timeUnixNano` — wall-clock timestamp as a **string** of nanoseconds
- `logRecord.attributes` — array of `{key, value: {stringValue}}` pairs including:
  - `run.id` — the run identifier
  - `run.working_dir`
  - `gardener.payload` — JSON-encoded string (4 KB truncated copy of the payload)
- `payload` — the full (possibly truncated) JSON payload, top-level field
- `payload.worker_id` — which worker emitted this event

### Rotation naming (`tools/gardener/src/log_retention.rs:19-75`)

```
otel-logs.jsonl       ← current (writable), always the newest events
otel-logs.1.jsonl     ← most recent rotation
otel-logs.2.jsonl
otel-logs.3.jsonl     ← oldest kept (keep=3)
```

Rotation fires **before** writing the triggering entry, so a single run's events can span multiple files. The current file always has the **newest** events.

### Key constraint

A run that was active when a 20 MB threshold was hit will have early events in `.3.jsonl` and recent events in `.jsonl`. Agents given only the current file will miss crucial context.

### Existing bin pattern

All binaries live in `tools/gardener/src/bin/<name>.rs` and are registered in `tools/gardener/Cargo.toml` as `[[bin]]` entries.

### Available dependencies (no additions needed)

- `serde_json` — JSON parsing
- `chrono` — timestamp formatting
- `tempfile` (dev) — test fixtures

---

## Desired End State

A single binary `otel-logs` with three subcommands:

1. **`index`** — Shows metadata for every log file (time range, event count, run IDs, worker IDs). Agents use this first to orient themselves before filtering.
2. **`filter`** — Streams matching events across all (or selected) files, filtered by run ID, worker ID, event type prefix, and/or time window. Output is bounded to avoid context explosion.
3. **`run-trace`** — Given a run ID, reconstructs the high-signal story of that run: state transitions, errors, first/last events, which files it spans.

---

## What We're NOT Doing

- No daemon/watch mode — these are pure read-only query tools
- No compression/decompression — files are plain JSONL
- No index file on disk — all queries scan files directly (files are small: ≤20 MB each, ≤4 files)
- No changes to the log format or rotation logic
- No new crate dependencies

---

## Implementation Approach

Single binary registered as `otel-logs` in Cargo.toml. Uses a hand-rolled arg parser (like the existing bins) or `clap` derive (already a dep). The core logic lives in a new library module `tools/gardener/src/log_query.rs` so it can be unit-tested independently from `main`.

### File discovery

```
fn discover_log_files(log_path: &Path) -> Vec<PathBuf>
```

Returns files in **chronological order** (oldest first):
`[otel-logs.3.jsonl, otel-logs.2.jsonl, otel-logs.1.jsonl, otel-logs.jsonl]`

Only includes files that exist. Skips missing rotation slots without error.

### Record parsing

```rust
struct LogRecord {
    source_file: PathBuf,
    line_number: usize,
    time_unix_nano: u64,       // 0 if missing/unparseable
    event_type: String,
    run_id: String,
    worker_id: String,         // "" if absent
    payload: serde_json::Value,
    raw_line: String,
}
```

Parsed from each JSONL line. Missing fields degrade gracefully (empty string / 0).

`run_id` is extracted from `logRecord.attributes` where `key == "run.id"` → `value.stringValue`.

---

## Phase 1: Core library module `log_query.rs`

### Changes required

**New file:** `tools/gardener/src/log_query.rs`

Functions to implement:

```rust
/// Returns log files in chronological order (oldest → newest).
pub fn discover_log_files(log_path: &Path) -> Vec<PathBuf>

/// Parse a single JSONL line into a LogRecord.
/// Returns None for blank lines or unparseable JSON.
pub fn parse_log_line(source: &Path, line_num: usize, line: &str) -> Option<LogRecord>

/// Compute FileIndex for a single log file (scan all lines).
pub fn index_file(path: &Path) -> FileIndex

pub struct FileIndex {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub line_count: usize,
    pub first_time_nano: u64,  // 0 if empty
    pub last_time_nano: u64,   // 0 if empty
    pub run_ids: Vec<String>,  // deduplicated, sorted
    pub worker_ids: Vec<String>,
}
```

**Register in lib.rs:** `pub mod log_query;`

### Success criteria

- `discover_log_files` returns files in chronological order with gaps handled
- `parse_log_line` returns `None` for blank/invalid lines, never panics
- `index_file` correctly extracts first/last timestamps, deduplicates run and worker IDs
- All functions have unit tests with `tempfile`-based fixtures

---

## Phase 2: `index` subcommand

### Behavior

```
otel-logs index [--log-path PATH]
```

Scans all rotated log files and prints a human-readable table (or JSON with `--json`):

```
FILE                     SIZE    LINES  FROM                TO                  RUNS         WORKERS
otel-logs.3.jsonl        18.2MB  41203  2026-02-28T14:00Z   2026-02-28T22:15Z   3 run(s)     5 worker(s)
otel-logs.2.jsonl        20.0MB  45102  2026-02-28T22:15Z   2026-03-01T06:30Z   2 run(s)     4 worker(s)
otel-logs.1.jsonl        19.8MB  44890  2026-03-01T06:31Z   2026-03-01T14:20Z   1 run(s)     3 worker(s)
otel-logs.jsonl           4.1MB   9201  2026-03-01T14:20Z   2026-03-01T16:45Z   1 run(s)     2 worker(s)
```

With `--json` flag, output is a JSON array of `FileIndex` objects.

**Default log path:** same resolution as `default_run_log_path` — `$GARDENER_LOG_PATH` → `$HOME/.gardener/otel-logs.jsonl` → CWD fallback.

### Success criteria

- Runs without error when no log files exist (prints empty table)
- `--json` output is valid JSON parseable by `serde_json`
- Timestamps formatted as RFC3339 in human mode

---

## Phase 3: `filter` subcommand

### Behavior

```
otel-logs filter [OPTIONS]

Options:
  --run-id <ID>           Filter to events for this run ID
  --worker-id <ID>        Filter to events for this worker
  --event-type <PREFIX>   Filter to event_type starting with PREFIX (e.g. "adapter.", "worker.activity")
  --since <NANO|RFC3339>  Exclude events before this time
  --until <NANO|RFC3339>  Exclude events after this time
  --max <N>               Stop after N matching events (default: 500)
  --tail                  Return last N events instead of first N
  --log-path <PATH>       Override log file path
  --json                  Output JSON array instead of newline-delimited records
  --files <N>             Only search the N most recent files (default: all)
```

Output (default): one JSON object per line, each with `source_file`, `line_number`, `time_rfc3339`, `event_type`, `run_id`, `worker_id`, `payload` fields.

With `--tail`, collects all matches then emits the last N — this is the common case for "what happened right before the failure."

### Implementation detail

Processes files oldest-to-newest. With `--tail`, buffers matches in a ring buffer of size `max`. Without `--tail`, emits and stops at `max`.

### Success criteria

- `--run-id` correctly spans multiple files for a run that crossed a rotation boundary
- `--max` hard-stops output (never emits more than N lines)
- `--tail` returns last N in chronological order
- Empty result exits 0 with no output (not an error)
- Invalid filter combination (e.g. bad timestamp) exits 1 with a clear message

---

## Phase 4: `run-trace` subcommand

### Behavior

```
otel-logs run-trace --run-id <ID> [--log-path PATH]
```

Produces a compact, high-signal trace of one run's lifecycle:

```json
{
  "run_id": "f4e6636ba759cf7d",
  "files_spanned": ["otel-logs.2.jsonl", "otel-logs.1.jsonl"],
  "first_event": { "time": "2026-03-01T06:31:00Z", "event_type": "run.started", ... },
  "last_event":  { "time": "2026-03-01T08:15:22Z", "event_type": "run.finished", ... },
  "duration_secs": 6262,
  "state_transitions": [
    { "time": "...", "worker_id": "worker-3", "state": "understand" },
    { "time": "...", "worker_id": "worker-3", "state": "plan" },
    ...
  ],
  "errors": [
    { "time": "...", "worker_id": "worker-3", "event_type": "worker.error", "summary": "..." }
  ],
  "worker_ids": ["worker-3", "worker-4"],
  "event_count": 1482
}
```

Error events are any record where `logRecord.severityNumber >= 17` (ERROR) or `event_type` contains `"error"` or `"failed"`.

### Success criteria

- Correctly identifies all files containing the run's events
- `state_transitions` lists all `worker.activity.state_changed` events in time order
- `errors` never exceeds 20 entries (take first 20)
- Exits 1 with clear message if run ID not found in any file

---

## Phase 5: Binary registration and integration

### Changes required

**New file:** `tools/gardener/src/bin/otel_logs.rs`

Uses `clap` derive (already a dep, same pattern as `friction-analysis`). Full `--help` at every level via `#[command(about = "...")]` on the top-level `Args` struct and each subcommand variant. Subcommands modeled as a `#[derive(Subcommand)]` enum. Main entry point delegates to `log_query` functions.

**Edit:** `tools/gardener/Cargo.toml`

```toml
[[bin]]
name = "otel-logs"
path = "src/bin/otel_logs.rs"
```

### Success criteria

- `cargo build --bin otel-logs` succeeds
- `otel-logs --help` prints usage
- `otel-logs index` runs against `.cache/gardener/otel-logs.jsonl` without error
- `otel-logs filter --run-id <real-id>` returns matching events from the live log

---

## Testing Strategy

### Unit tests (in `log_query.rs`)

| Test | What it verifies |
|------|-----------------|
| `discover_log_files_chronological_order` | Returns oldest→newest, skips missing slots |
| `discover_log_files_only_current` | Works when no rotations exist |
| `parse_log_line_valid` | Extracts all fields from a well-formed line |
| `parse_log_line_missing_fields` | Degrades gracefully, returns `Some` with empty strings |
| `parse_log_line_blank` | Returns `None` |
| `parse_log_line_invalid_json` | Returns `None` |
| `index_file_timestamps` | Correct first/last from timeUnixNano |
| `index_file_run_ids_deduplicated` | No duplicates in run_ids list |
| `index_file_empty_file` | Returns zero-valued FileIndex |

### Integration tests (in `tests/otel_log_query.rs`)

Using `assert_cmd` and `tempfile`:

| Test | What it verifies |
|------|-----------------|
| `filter_run_id_spans_rotation_boundary` | Events from two files, filtered by run_id, all returned |
| `filter_max_limits_output` | `--max 5` returns exactly 5 lines |
| `filter_tail_returns_last_n` | `--tail` with `--max 3` returns chronologically last 3 |
| `run_trace_identifies_files_spanned` | Correct `files_spanned` list |
| `run_trace_missing_run_exits_1` | Exit code 1 when run not found |
| `index_json_output_is_valid` | `--json` output parses as JSON array |
| `index_empty_directory` | Runs cleanly with no files |

### Manual smoke test

```bash
cargo build --bin otel-logs
./target/debug/otel-logs index
./target/debug/otel-logs filter --run-id $(./target/debug/otel-logs index --json | jq -r '.[0].run_ids[0]') --max 20
```

---

## References

- `tools/gardener/src/logging.rs` — log format, field names, rotation trigger
- `tools/gardener/src/log_retention.rs` — rotation naming convention
- `tools/gardener/src/bin/do_task.rs` — bin entry point pattern
- `tools/gardener/Cargo.toml` — dependency list, bin registration
