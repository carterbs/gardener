---
name: log-debugging
description: 'Debug Gardener runtime failures by joining git worktree/branch/commit context with OTEL JSONL logs. Use for: reading and filtering malformed or high-volume `otel-logs.jsonl`, isolating failed runs by `run.id`/worker, tracing failure events, and mapping runtime context from log entries back to git run/worktree state.'
---

# Gardener Log Debugging

## Mandatory approach (read first)

- ALWAYS start with `otel-logs run-trace --run-id <RUN_ID>` once a run id is known.
- ALWAYS use `otel-logs filter --run-id ... --event-type ...` to narrow scope before any raw extraction.
- NEVER paste large raw JSON streams into context. Pull only the specific event families needed for the current hypothesis.
- Use broad `jq`/`rg` over full log files only as a last resort after `run-trace` + targeted `filter` are insufficient.

## Core workflow

1. Start with run failure signal (dashboard entry, test failure, or failed worker action).
2. Resolve the matching run id.
3. Pull matching events with `otel-logs` (`run-trace` first, then targeted `filter`).
4. Narrow to failure-relevant events and build a timeline.
5. Map each failure to git context using `run.working_dir` and worker/task metadata.
6. Reproduce using the exact worktree and command in payload if present.

## OTEL utility to use

Use the `otel-logs` binary:

```bash
export OTEL_BIN=${OTEL_BIN:-cargo run -q -p gardener --bin otel-logs --}
```

Default log path resolution:
- `$GARDENER_LOG_PATH` if set
- otherwise `$HOME/.gardener/otel-logs.jsonl`

Only pass `--log-path` when you intentionally want a non-default log file:
`$OTEL_BIN --log-path /tmp/otel-logs.jsonl ...`

## Log sanity and discovery

- Show available rotated files:
  - `$OTEL_BIN index`
- Build high-signal lifecycle trace:
  - `$OTEL_BIN run-trace --run-id <RUN_ID>`
- Fetch scoped event slices:
  - `$OTEL_BIN filter --run-id <RUN_ID> --event-type <PREFIX> --max 200`

## Failure-to-logs workflow

1. Find likely failures in the current log stream:
  - `$OTEL_BIN filter --event-type run.failed --tail --max 50`
  - `$OTEL_BIN filter --event-type agent.turn.finished --tail --max 100`
2. Capture the run id from a failure line and assign:
  - `RUN=...`
3. Reconstruct full context for that run:
  - `$OTEL_BIN run-trace --run-id "$RUN"`
4. Pull only the event families needed for the active hypothesis:
  - `$OTEL_BIN filter --run-id "$RUN" --event-type backlog.task --max 200`
  - `$OTEL_BIN filter --run-id "$RUN" --event-type merge_worker --max 200`
  - `$OTEL_BIN filter --run-id "$RUN" --event-type friction_analysis --max 200`
5. Use raw extraction only if targeted filters are insufficient.

## Git-to-logs workflow

- From a known worktree:
  - `workdir="/Users/bradcarter/Documents/Dev/gardener/.worktrees/worker-1..."`
  - `git -C "$workdir" rev-parse --short HEAD`
  - `git -C "$workdir" status --short`
- Resolve commit context from a known run id using targeted run trace and event filter output.

## Useful failure clusters

- adapter parse issues:
  - `$OTEL_BIN filter --run-id "$RUN" --event-type stdout_non_json --max 200`
- process spawn failures:
  - `$OTEL_BIN filter --run-id "$RUN" --event-type process.spawn --max 200`
- terminal transitions:
  - `$OTEL_BIN filter --run-id "$RUN" --event-type terminal_result --max 200`

## One-command triage

Use `RUN` to print a dense run audit with bounded output:

```bash
RUN=...
$OTEL_BIN run-trace --run-id "$RUN"
$OTEL_BIN filter --run-id "$RUN" --event-type backlog.task --max 100
$OTEL_BIN filter --run-id "$RUN" --event-type merge_worker --max 100
```
