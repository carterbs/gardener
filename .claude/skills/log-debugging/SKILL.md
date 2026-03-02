---
name: log-debugging
description: 'Debug Gardener runtime failures by joining git worktree/branch/commit context with OTEL JSONL logs. Use for: reading and filtering malformed or high-volume `otel-logs.jsonl`, isolating failed runs by `run.id`/worker, tracing failure events, and mapping runtime context from log entries back to git run/worktree state.'
---

# Gardener Log Debugging

## Core workflow

1. Start with run failure signal (dashboard entry, test failure, or failed worker action).
2. Resolve the matching run id.
3. Pull matching events with the OTEL log utility.
4. Narrow to failure-relevant events and build a timeline.
5. Map each failure to git context using `run.working_dir` and worker/task metadata.
6. Reproduce using the exact worktree and command in payload if present.

## Log-query utility to use

Use the `log-query` binary to avoid brittle manual `jq` pipelines:

```bash
export LOG_QUERY_BIN=${LOG_QUERY_BIN:-"cargo run --manifest-path tools/gardener/Cargo.toml --bin log-query --"}
export LOG_PATH=${GARDENER_LOG_PATH:-$HOME/.gardener/otel-logs.jsonl}
```

## Log sanity and discovery

- Show top-level summary (events, runs, workers):
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" stats`
- Fetch matching events:
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" events --run-id <RUN_ID> --limit 100`
- Build a compact timeline:
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" timeline --run-id <RUN_ID> --limit 200`

## Failure-to-logs workflow

1. Find likely failures in the current log stream:
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" events --contains '"terminal":"failure"' --limit 50`
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" events --event-type "agent.turn.finished" --limit 50`
2. Capture the run id from a failure line and assign:
  - `RUN=...`
3. Reconstruct full context for that run:
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" timeline --run-id "$RUN"`
4. Pull raw rows when payload fields are needed:
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" events --run-id "$RUN" --raw --contains '"terminal"' --limit 200 | jq -R 'fromjson? // empty | "\(.logRecord.timeUnixNano) \(.event_type) run=\(.payload.run_id // \"\") worker=\(.payload.worker_id // \"\") terminal=\(.payload.terminal // \"\") cmd=\(.payload.command // \"\")"'`

## Git-to-logs workflow

- From a known worktree:
  - `workdir="/Users/bradcarter/Documents/Dev/gardener/.worktrees/worker-1..."`
  - `git -C "$workdir" rev-parse --short HEAD`
  - `git -C "$workdir" status --short`
- Find run candidates from workdir path:
  - `WORKDIR="/Users/bradcarter/Documents/Dev/gardener/.worktrees/worker-1..."`
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" events --contains "$WORKDIR" --raw | head -n 200`
- Resolve commit context from a known run id:
  - `RUN=...`
  - `gitroot=$($LOG_QUERY_BIN --log-path "$LOG_PATH" events --run-id "$RUN" --contains '"run.working_dir"' --raw | jq -R 'fromjson? // empty | .logRecord.attributes[]? | select(.key=="run.working_dir") | .value.stringValue' | head -n 1)`
  - `git -C "$gitroot" rev-parse --short HEAD`
  - `git -C "$gitroot" log --oneline -n 5`

## Useful failure clusters

- adapter parse issues:
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" events --event-type "stdout_non_json"`
- process spawn failures:
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" events --event-type "process_spawn"`
- terminal transitions:
  - `$LOG_QUERY_BIN --log-path "$LOG_PATH" events --event-type "terminal_result"`

## One-command triage

Use `RUN` to print a dense run audit:

```bash
RUN=...
$LOG_QUERY_BIN --log-path "$LOG_PATH" timeline --run-id "$RUN"
$LOG_QUERY_BIN --log-path "$LOG_PATH" events --run-id "$RUN" --contains '"terminal":"failure"' --raw --limit 50 \
  | jq -R 'fromjson? // empty | "\(.logRecord.timeUnixNano) \(.event_type) worker=\(.payload.worker_id // \"\") run_dir=\(.payload.working_dir // \"\") cmd=\(.payload.command // \"\")"'
```
