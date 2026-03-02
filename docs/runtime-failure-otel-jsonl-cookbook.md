# OTEL JSONL Triage Cookbook for Runtime Failures

Use this flow when Gardener has a runtime failure and you need a fast, reproducible diagnosis from OTEL logs.

## 1) Find fresh failure signals

```bash
export LOG_QUERY_BIN=${LOG_QUERY_BIN:-log-query}
```

- Search current failure signals:

```bash
$LOG_QUERY_BIN events --contains '"terminal":"failure"' --limit 80
```

- Include turn completion and adapter parse failures:

```bash
$LOG_QUERY_BIN events --contains '"terminal":"failure"\|"stdout_non_json"\|"process_error"' --limit 80
```

- Show event distribution for a broad view:

```bash
$LOG_QUERY_BIN stats
```

## 2) Resolve the failing run id

From a target failure line, extract `run.id` from attributes:

```bash
$LOG_QUERY_BIN events --contains '"terminal":"failure"' --raw --limit 1 \
  | jq -R 'fromjson? // empty
    | .logRecord.attributes[]?
    | select(.key == "run.id")
    | .value.stringValue' \
  | head -n 1
```

Assign it as `RUN`:

```bash
RUN=<run_id>
```

## 3) Build a minimal timeline for the run

```bash
$LOG_QUERY_BIN timeline --run-id "$RUN" --limit 200
```

## 4) Pull raw payload for root-cause evidence

```bash
$LOG_QUERY_BIN events --run-id "$RUN" --raw --contains '"terminal"' --limit 400 \
  | jq -R 'fromjson? // empty
    | "\(.logRecord.timeUnixNano) \(.event_type) run=\(.payload.run_id // "") worker=\(.payload.worker_id // "") terminal=\(.payload.terminal // "") event=\(.payload.event_type // "") cmd=\(.payload.command // "")"'
```

For non-JSON subprocess output, isolate parser failures:

```bash
$LOG_QUERY_BIN events --run-id "$RUN" --raw --event-type "adapter.codex.stdout_non_json" --limit 200 \
  | jq -R 'fromjson? // empty
    | "\(.logRecord.timeUnixNano) \(.payload.line // "")"'
```

## 5) Map failure back to git/worktree context

- Resolve the worktree path:

```bash
WORKDIR=$($LOG_QUERY_BIN events --run-id "$RUN" --raw --contains '"run.working_dir"' \
  | jq -R 'fromjson? // empty
    | .logRecord.attributes[]?
    | select(.key == "run.working_dir")
    | .value.stringValue' \
  | head -n 1)
```

- Inspect local git state at that path:

```bash
git -C "$WORKDIR" status --short
git -C "$WORKDIR" rev-parse --short HEAD
```

- Trace related commits:

```bash
git -C "$WORKDIR" log --oneline -n 12
```

## 6) Useful runtime failure clusters

- Adapter parse issues:
  - `agent.turn.finished`
  - `adapter.codex.stdout_non_json`
  - `adapter.claude.stdout_non_json`
- Process launch and sandbox issues:
  - `process_spawn`
  - `process_error`
- Turn lifecycle transitions:
  - `worker.task.process_error`
  - `agent.turn.started`
  - `agent.turn.finished`

## 7) Quick live follow-up

- Watch rotating logs while reproducing or rerunning:

```bash
./scripts/watch-otel-logs.sh
```

- Point tailing at a custom file:

```bash
GARDENER_LOG_PATH=/tmp/otel-logs.jsonl ./scripts/watch-otel-logs.sh
```

## 8) One-command run audit

```bash
RUN=<run_id>
$LOG_QUERY_BIN timeline --run-id "$RUN" --limit 200
$LOG_QUERY_BIN events --run-id "$RUN" --contains '"terminal":"failure"' --raw --limit 80 \
  | jq -R 'fromjson? // empty
    | "\(.logRecord.timeUnixNano) worker=\(.payload.worker_id // "") dir=\(.payload.working_dir // .payload.run_id // "") terminal=\(.payload.terminal // "")"'
```
