---
name: log-debugging
description: Debug Gardener runtime failures by joining git worktree/branch/commit context with OTEL JSONL logs. Use for: reading and filtering malformed or high-volume `otel-logs.jsonl`, isolating failed runs by `run.id`/worker, tracing failure events, and mapping runtime context from log entries back to git run/worktree state.
---

# Gardener Log Debugging

## Core workflow

1. Start with run failure signal (dashboard entry, test failure, or failed worker action).
2. Resolve the matching run id from nearby logs.
3. Pull all events for that run id into a compact timeline.
4. Filter to failure-relevant event types.
5. Map each failure to git context using `run.working_dir` and worker/task metadata.
6. Reproduce using the exact worktree and command in `payload.command` if present.

## Log file locations

- Live log path: `~/.gardener/otel-logs.jsonl`
- Optional archive paths follow same JSONL shape; replace path accordingly.

## Parse sanity checks with `jq`

Use this in a clean terminal where `jq` is available.

- Parseable lines count:
  - `total=$(wc -l < ~/.gardener/otel-logs.jsonl)`
  - `parsed=$(jq -R 'fromjson? // empty' ~/.gardener/otel-logs.jsonl | wc -l)`
  - `printf 'parsed=%s of %s\n' "$parsed" "$total"`
- Drop malformed rows only when needed:
  - `jq -R 'fromjson? // empty' ~/.gardener/otel-logs.jsonl > /tmp/otel-logs.clean`
- See top event distribution:
  - `jq -R 'fromjson? // empty | .event_type' ~/.gardener/otel-logs.jsonl | sort | uniq -c | sort -nr | head`

## Common jq field patterns

- Extract canonical `run.id` and `run.working_dir` from a row:
  - `def run_attrs: .logRecord.attributes[] | .key as $k | select($k=="run.id" or $k=="run.working_dir");`
- Show only failed turn-finishing records:
  - `jq -R 'fromjson? // empty | select(.payload.terminal == "failure") | {time: .logRecord.timeUnixNano, run_id: (.logRecord.attributes[] | select(.key=="run.id") | .value.stringValue), worker: .payload.worker_id, terminal: .payload.terminal, event: .event_type, msg: (.logRecord.body | .stringValue // "")}' ~/.gardener/otel-logs.jsonl`
- Filter by event type:
  - `... | select(.event_type == "agent.turn.finished")`
  - `... | select(.event_type == "backlog.task.failed")`
  - `... | select(.event_type | startswith("adapter.codex") or startswith("adapter.claude"))`
- Filter by worker:
  - `... | select(.payload.worker_id == "worker-1")`
- Filter by raw event payload (`assistant`/`user`/etc.):
  - `... | select(.payload.raw_type == "assistant")`
- Filter by command:
  - `... | select(.payload.command? // "" | contains("npm run gardener:run"))`
- Show a concise one-line timeline for a specific run:
  - `RUN=fe4f5d6...`
  - `jq -R --arg RUN "$RUN" 'fromjson? // empty | select((.logRecord.attributes[] | select(.key=="run.id") | .value.stringValue) == $RUN) | {t:.logRecord.timeUnixNano, event:.event_type, terminal:.payload.terminal, worker:.payload.worker_id, cmd:.payload.command, msg:(.logRecord.body.stringValue // .logRecord.body)}' ~/.gardener/otel-logs.jsonl`

## Failure-to-logs workflow

1. Identify a failure in CI/local run output.
2. Use timestamp to locate nearby log entries:
   - `TZ=UTC jq -R 'fromjson? // empty | select(.logRecord.timeUnixNano | tonumber > 1772255489600000000)' ~/.gardener/otel-logs.jsonl | head -n 200`
3. Find the first likely failure line:
   - `... | select(.payload.terminal == "failure")`
4. Pull its `run.id` and `run.working_dir`:
   - `... | select(.payload.terminal == "failure") | {run_id, event_type, working_dir: (.logRecord.attributes[] | select(.key=="run.working_dir") | .value.stringValue), worker:.payload.worker_id}`
5. Reconstruct sequence for that run id and narrow root cause by stepping backward to the last `agent.turn.started` / `adapter.*.turn_start`.

## Git-to-logs workflow

- From a known git worktree (worker checkout path):
  - `workdir="/Users/bradcarter/Documents/Dev/gardener/.worktrees/worker-1..."`
  - `git -C "$workdir" rev-parse --short HEAD`
  - `git -C "$workdir" status --short`
- Find matching logs for that worktree:
  - `RUN_DIR="/Users/bradcarter/Documents/Dev/gardener/.worktrees/worker-1..."`
  - `jq -R --arg RUN_DIR "$RUN_DIR" 'fromjson? // empty | select((.logRecord.attributes[] | select(.key=="run.working_dir") | .value.stringValue) == $RUN_DIR) | {t:.logRecord.timeUnixNano, event:.event_type, run_id:(.logRecord.attributes[] | select(.key=="run.id") | .value.stringValue), worker:.payload.worker_id, terminal:.payload.terminal}' ~/.gardener/otel-logs.jsonl`
- From a known `run.id`, jump to git commit context if `run.working_dir` appears:
  - `RUN=...`
  - `gitroot=$(jq -R --arg RUN "$RUN" 'fromjson? // empty | select((.logRecord.attributes[] | select(.key=="run.id") | .value.stringValue) == $RUN) | first((.logRecord.attributes[] | select(.key=="run.working_dir") | .value.stringValue))' ~/.gardener/otel-logs.jsonl)`
  - `git -C "$gitroot" rev-parse --short HEAD`
  - `git -C "$gitroot" log --oneline -n 5`

## Useful failure clusters

- adapter output parse issue:
  - `... | select(.event_type|endswith("stdout_non_json"))`
- process spawn failures:
  - `... | select(.event_type|endswith("process_spawn"))`
- terminal result/state transitions:
  - `... | select(.event_type == "adapter.codex.terminal_result" or .event_type == "adapter.claude.terminal_result")`

## One-command triage template

- Given `RUN`, print a dense audit for that run:

```bash
RUN=...;
jq -R --arg RUN "$RUN" '
  fromjson? // empty
  | select((.logRecord.attributes[] | select(.key=="run.id") | .value.stringValue) == $RUN)
  | "\(.logRecord.timeUnixNano) \(.event_type) worker=\(.payload.worker_id // "") terminal=\(.payload.terminal // "") run_dir=\((.logRecord.attributes[] | select(.key=="run.working_dir") | .value.stringValue // "") ) cmd=\((.payload.command // "") )"
' ~/.gardener/otel-logs.jsonl
```

