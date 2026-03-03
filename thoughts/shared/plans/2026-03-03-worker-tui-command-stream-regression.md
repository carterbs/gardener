# Plan: Restore Worker TUI Command Updates After Direct Streaming Cutover

## Overview
`main` currently drops nearly all live command updates before they reach the UI. The regression came in with the direct-streaming path (`e2ca211`, later merged/refined in `366141f`) and is caused by command payload extraction logic that no longer matches real adapter event shapes (including Codex and Claude stream-json variants).

## Current State Analysis
- Pre-cutover (`e2ca211^`), worker command rows were populated from log polling (`append_worker_tool_commands`) and the polling parser accepted `command`, `input.command`, recursive `item.*`, and recursive `payload.*` shapes.
- Current `main` streams events directly from worker callbacks, but command extraction in worker runtime is narrower and drops events that do not match its limited shape.

Evidence:
- Direct-stream extraction currently used by runtime:
  - `emit_adapter_tool_event` drops event when parse returns `None`: `tools/gardener/src/worker.rs:147-163`
  - narrow extractor (`payload.payload.inputs|input.command|value`, then `message|text|content` only): `tools/gardener/src/worker.rs:166-196`
- Legacy polling parser (worked pre-cutover) accepts broader shapes:
  - `tools/gardener/src/logging.rs:493-525`
- Codex raw events carry commands in `item.command` (not under top-level `payload.inputs`):
  - event mapping keeps raw event payload: `tools/gardener/src/protocol.rs:43-61`
  - codex stream examples in runtime logs include `payload.item.command` for `item.started/item.completed`
- Existing tests pass but do not cover this shape mismatch:
  - `recent_worker_tool_commands_collects_tool_events` passes (polling parser): `tools/gardener/src/logging.rs:933-956`
  - `apply_pool_stream_event_updates_doing_worker_from_live_events` passes (pool apply logic only): `tools/gardener/src/worker_pool.rs:2107-2186`

## OTEL Evidence (Exhaustive)
Sources queried:
- `.cache/gardener/otel-logs.jsonl`
- `.cache/gardener/otel-logs.jsonl.bad-before-fix`

### Codex event shapes in OTEL
Observed in `.cache/gardener/otel-logs.jsonl.bad-before-fix`:
- Total `adapter.codex.event` records: `2322`
- Records with command at raw path `payload.item.command`: `1323`
- Records with command at raw path `payload.input.command`: `0`
- Records with command at raw path `payload.inputs.command`: `0`

Raw-type breakdown (same source):
- `item.started`: `793` total, `719` with `payload.item.command`, `74` without
- `item.completed`: `1318` total, `604` with `payload.item.command`, `714` without
  - commandless `item.completed` are mostly non-command item types (`reasoning`, `agent_message`, `file_change`, `collab_tool_call`)
- `thread.started`: `62` total, `0` with command
- `turn.started`: `88` total, `0` with command
- `turn.completed`: `61` total, `0` with command

Line-numbered OTEL examples (same file):
- Line `250` (`item.started`, command-carrying):
  - `payload.payload.item.command = "/bin/zsh -lc 'git status --short && git fetch origin main && git rebase origin/main'"`
- Line `251` (`item.completed`, command-carrying):
  - `payload.payload.item.command = "/bin/zsh -lc 'git status --short && git fetch origin main && git rebase origin/main'"`
- Line `195` (`item.completed`, commandless):
  - `payload.payload.item.type = "reasoning"` and no `item.command`
- Lines `189/190/197` (`thread.started` / `turn.started` / `turn.completed`):
  - no command payload fields

### Claude event shapes in OTEL
Observed in `.cache/gardener/otel-logs.jsonl.bad-before-fix`:
- Total `adapter.claude.event` records: `1216`
- Records with command at raw path `payload.message.content[*].input.command`: `364`
- Records with command at raw path `payload.input.command`: `0`
- Records with command at raw path `payload.command`: `0`
- Records with top-level adapter field `command`: `0`

Raw-type breakdown (same source):
- `assistant`: `727` total, `364` with `message.content[*].input.command`, `363` without
- `user`: `365` total, `0` with command path (mostly `tool_result` / text)
- `rate_limit_event`: `42` total, `0` with command path
- `system`: `41` total, `0` with command path
- `result`: `41` total, `0` with command path

Line-numbered OTEL examples (same file):
- Line `2` (`assistant`, command-carrying tool use):
  - `payload.payload.message.content[0].type = "tool_use"`
  - `payload.payload.message.content[0].name = "Bash"`
  - `payload.payload.message.content[0].input.command = "git -C ... rev-parse --abbrev-ref HEAD"`
- Line `62` (`assistant`, tool use but commandless):
  - `name = "Read"` and `input = {"file_path":".../tools/gardener/src/lib.rs"}` (no `input.command`)
- Line `7` (`assistant`, non-tool command content):
  - `content[0].type = "thinking"` (no command expected)
- Line `1` (`user`, tool result):
  - `content[0].type = "tool_result"` with `tool_use_result` payload; no command path

### Cross-check on active log file
Observed in `.cache/gardener/otel-logs.jsonl`:
- Codex totals are consistent (`2320` total; `1323` with raw `payload.item.command`)
- Claude totals are consistent (`1216` total; `364` with raw `payload.message.content[*].input.command`)

Implication:
- Real command-bearing Codex events are predominantly `item.command`.
- Real command-bearing Claude events are `message.content[*].input.command`.
- Current direct-stream parser in `worker.rs` does not traverse either shape, so these events are dropped before UI emission.

## Desired End State
- Live streaming path extracts command text from the same payload shapes the old polling path supported (at minimum: `command`, `input.command`, recursive `item`, recursive `payload`) plus Claude stream-json `message.content[*].input.command`.
- A regression test fails on current `main` and passes with the fix.
- No change to higher-level TUI rendering behavior is required for this bug (ordering/truncation stays as implemented).

## What We Are Not Doing
- No redesign of TUI display format, command limits, or compact-mode rendering.
- No changes to worker state-transition gating in this fix unless required by failing test evidence.
- No broader refactor of merge/worker threading model.

## Implementation Approach
### Phase 1: Add Failing Regression Tests (Required)
Goal: codify the exact missing command-shape behavior in the direct-stream runtime path.

Changes:
- Add a new unit test in `tools/gardener/src/worker.rs` test module (near existing parser-oriented tests) that exercises `format_adapter_event_command` via payload shapes currently dropped.
- Minimum failing case (must fail on current `main`):
  - input payload shaped like codex tool event:
    - `{"type":"item.started","item":{"type":"command_execution","command":"git status"}}`
  - expected: `Some("git status")` (or prefixed form when kind/raw_type applies)
- Add explicit Claude failing case (must fail on current `main`):
  - input payload shaped like Claude assistant tool-use envelope:
    - `{"type":"assistant","message":{"content":[{"type":"tool_use","input":{"command":"cargo test"}}]}}`
  - expected: extracted command contains `cargo test`.
- Add one secondary assertion for nested recursion parity with polling parser:
  - `{"payload":{"item":{"command":"cargo test"}}}` should also extract.

Success criteria:
- `cargo test -p gardener <new_test_name>` fails on current `main` before fix.

Confirmation gate:
- Stop and confirm failing assertion text points to command extraction path, not unrelated formatting.

### Phase 2: Fix Extraction Logic in Streaming Path
Goal: make live-stream extraction parity-match the shapes accepted by old polling parser.

Changes:
- Update `extract_payload_command` in `tools/gardener/src/worker.rs` to include, in order:
  - top-level `command`
  - top-level `input.command`
  - recursive `item` descent
  - Claude `message.content[*].input.command` descent
  - existing `message|text|content`
  - recursive `payload` descent
- Keep newline escaping and truncation behavior unchanged (`PROMPT_LINE_COMMAND_LIMIT`, `truncate_utf8`).
- Keep `emit_adapter_tool_event` behavior unchanged except that it now receives parsed command text for real events.

Success criteria:
- New regression test passes.
- Existing nearby worker tests still pass.

Confirmation gate:
- Verify command extraction function now accepts both codex and legacy nested payload shapes.

### Phase 3: Validate End-to-End and Guard Against Drift
Goal: ensure fix is stable and observed at runtime.

Changes:
- Run focused tests:
  - new worker regression test
  - `cargo test -p gardener apply_pool_stream_event_updates_doing_worker_from_live_events`
  - `cargo test -p gardener recent_worker_tool_commands_collects_tool_events`
- Manual smoke:
  - run `cargo run -p gardener --bin gardener -- --quit-after 1 --config <path>`
  - confirm worker card shows live `Commands:` updates during agent activity.

Success criteria:
- All targeted tests pass.
- Manual run shows at least one live command line while worker is active.

## Testing Strategy
Automated:
- Unit: new regression tests in `worker.rs` for codex `item.command` and Claude `message.content[*].input.command` shapes.
- Regression: existing worker_pool/logging tests above remain green.

Manual:
- Single-task runtime execution with TUI visible; verify command stream populates without relying on log polling.

## References
- `tools/gardener/src/worker.rs:147-217`
- `tools/gardener/src/logging.rs:493-525`
- `tools/gardener/src/worker_pool.rs:1067-1130`
- `tools/gardener/src/protocol.rs:43-61`
- Commits: `e2ca211` (initial direct streaming), `366141f` (polling removal/state sink follow-through)
