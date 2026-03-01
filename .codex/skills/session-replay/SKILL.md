---
name: session-replay
description: 'Generate a session recording file and reproduction test from a Gardener TUI error. Use when a worker fails with a visible error (e.g. "process error: missing turn.completed or turn.failed event") and you want to reproduce it deterministically via the replay system.'
---

# Session Replay from TUI Error

Turns a Gardener runtime failure into a deterministic session recording + test.

## Inputs

Paste the full TUI error block. It looks like:

```
Error: process error: <message>
Last 15 logs for worker-<N>:
  ...
  adapter.codex.stdout_non_json  {"error":"...","line":"..."}
  ...
```

The key fields to extract:
- **worker id** — `worker-<N>` from the error header
- **task id** — shown in the dashboard or log payload
- **error message** — the `process error: <message>` text
- **run.id** — must be looked up from otel logs (see below)

---

## Step 1 — Find the run.id

```bash
# Find the run.id by matching the error text near the failure timestamp
grep "missing turn.completed\|<your error text>" ~/.gardener/otel-logs.jsonl \
  | jq -r '(.logRecord.attributes[]? | select(.key=="run.id") | .value.stringValue) // empty' \
  | tail -1
```

Or search by worker and event type:
```bash
jq -R 'fromjson? // empty
  | select(.event_type == "worker.task.process_error")
  | {run_id: (.logRecord.attributes[]? | select(.key=="run.id") | .value.stringValue),
     worker: .payload.worker_id,
     error: .logRecord.body.stringValue}' \
  ~/.gardener/otel-logs.jsonl | tail -5
```

---

## Step 2 — Extract subprocess stdout

```bash
RUN=<run_id>
jq -R --arg RUN "$RUN" '
  fromjson? // empty
  | select((.logRecord.attributes[]? | select(.key=="run.id") | .value.stringValue) == $RUN)
  | select(.event_type == "adapter.codex.stdout_non_json" or .event_type == "adapter.claude.stdout_non_json")
  | {t: .logRecord.timeUnixNano, line: .payload.line, error: .payload.error}
' ~/.gardener/otel-logs.jsonl
```

The `line` field contains the raw subprocess stdout that failed to parse.

**Pattern to watch for:** If each successive entry shows a *growing* accumulation of
JSON objects (entry 2 includes entry 1's content, entry 3 includes entries 1+2, etc.),
this is the `append_and_flush_lines` bug in `runtime/mod.rs` — `[..end]` instead of
`[cursor..end]`. The last failing entry contains the terminal event (`turn.completed`)
that was never parsed.

Also extract the successful events to understand the full stdout sequence:
```bash
jq -R --arg RUN "$RUN" '
  fromjson? // empty
  | select((.logRecord.attributes[]? | select(.key=="run.id") | .value.stringValue) == $RUN)
  | select(.event_type | startswith("adapter.codex") or startswith("adapter.claude"))
  | "\(.logRecord.timeUnixNano) \(.event_type)"
' ~/.gardener/otel-logs.jsonl
```

---

## Step 3 — Build the recording file

Create `tools/gardener/tests/fixtures/recording_<run_id>.jsonl`.

The file is JSONL — one `RecordEntry` per line. Minimum required entries:

```jsonl
{"type":"session_start","run_id":"<run_id>","recorded_at_unix_ns":<ts>,"gardener_version":"reconstructed-from-otel-logs","config_snapshot":{}}
{"type":"backlog_snapshot","tasks":[{"task_id":"<task_id>","kind":"Maintenance","title":"<title>","details":"","rationale":"","scope_key":"gardener","priority":"P1","status":"ready","last_updated":0,"lease_owner":null,"lease_expires_at":null,"source":"manual","related_pr":null,"related_branch":null,"attempt_count":1,"created_at":0}]}
{"type":"backlog_mutation","seq":1,"timestamp_ns":<ts>,"worker_id":"<worker_id>","operation":"claim_next","task_id":"<task_id>","result_ok":true}
{"type":"process_call","seq":10,"timestamp_ns":<ts>,"worker_id":"<worker_id>","thread_id":"<worker_id>","request":{"program":"codex","args":[],"cwd":null},"result":{"exit_code":0,"stdout":"<stdout_json_escaped>","stderr":"","stdout_truncated":false},"duration_ns":0}
{"type":"session_end","completed_tasks":0,"total_duration_ns":0}
```

For the `stdout` field: reconstruct the full subprocess stdout from the otel log entries.
- Successful events each represent one `\n`-terminated JSON object that parsed OK.
- The failing `stdout_non_json` entries' `line` field shows the accumulated buggy slices —
  the *last* failing entry contains all the objects that arrived as one chunk. Use that to
  reconstruct the chunk: split by `\n`, remove duplicates, keep unique objects in order.
- JSON-escape the full stdout string for embedding in the JSONL file
  (use `jq -Rs '.' <<< "$stdout"` or escape manually).

---

## Step 4 — Add the replay test

In `tools/gardener/tests/replay_integration.rs`, add:

```rust
/// Session replay for:
///   run.id:  <run_id>
///   worker:  <worker_id>
///   task:    <task_id>
///   error:   "<error message>"
///
/// Recording reconstructed from otel-logs.jsonl.
#[test]
fn session_replay_<short_description>() {
    let recording_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/recording_<run_id>.jsonl");
    let recording = SessionRecording::load(&recording_path)
        .expect("load recording");

    let runner = ReplayProcessRunner::from_recording(&recording, "<worker_id>");

    let ctx = AdapterContext {
        worker_id: "<worker_id>".to_string(),
        session_id: "<run_id>".to_string(),
        sandbox_id: String::new(),
        model: "<model>".to_string(),
        cwd: std::path::PathBuf::from("/tmp"),
        prompt_version: "v1".to_string(),
        context_manifest_hash: String::new(),
        output_schema: None,
        output_file: Some(std::path::PathBuf::from("/tmp/codex-last-message.json")),
        permissive_mode: false,
        max_turns: None,
    };

    let err = CodexAdapter  // or ClaudeAdapter
        .execute(&runner, &ctx, "prompt", None)
        .expect_err("should reproduce the production error");

    assert!(
        err.to_string().contains("<error message>"),
        "expected production error, got: {err}"
    );
}
```

Run it: `cargo test -p gardener session_replay_<short_description>`

It should **pass** (the error fires as expected). When the underlying bug is fixed, the
test will **fail** — confirming the fix resolved the reproduction case.

---

## Key architectural notes

- `ReplayProcessRunner::wait_with_line_stream` mirrors `ProductionProcessRunner`'s
  `append_and_flush_lines` verbatim, including any bugs. Both must be updated together
  when fixing the underlying issue.
- `append_run_log` no-ops when no logger is initialized — replay tests do not pollute
  `~/.gardener/otel-logs.jsonl`.
- Recording files live in `tools/gardener/tests/fixtures/` and are committed to the repo.
- The `SessionRecording` type is in `src/replay/replayer.rs`; adapters are in `src/agent/`.

---

## Supported adapter types

| TUI error prefix | Adapter | Import |
|---|---|---|
| `adapter.codex.*` | `CodexAdapter` | `use gardener::agent::codex::CodexAdapter;` |
| `adapter.claude.*` | `ClaudeAdapter` | `use gardener::agent::claude::ClaudeAdapter;` |
