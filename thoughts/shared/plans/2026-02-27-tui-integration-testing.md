# TUI Integration Testing Plan

## Overview

Build a comprehensive integration test suite for Gardener's TUI: hotkeys, triage flow,
rendering, and error handling. The strategy uses fixture NDJSON files (sourced from real
`claude`/`codex` runs in this branch) to drive `FakeProcessRunner`, extends `FakeTerminal`
for rendered-content assertions, and adds screen-content verification to the existing PTY
e2e tests.

## Current State

### What exists and works
- `FakeProcessRunner` (`runtime/mod.rs:934`) — queue-based, captures `spawned()`, `waits()`, `kills()`
- `FakeTerminal` (`runtime/mod.rs:828`) — captures `written_lines()`, `drawn_frames()`, `dashboard_draw_count()`, `report_draws()`, `shutdown_screens()`; key injection via `enqueue_keys()`
- `render_dashboard()` (`tui.rs:661`) — calls `draw_dashboard_frame()` against a `TestBackend` backed by a `Buffer`, returns a rendered string; already used in `tui_harness_test.rs`
- `hotkey_pty_e2e.rs` — 4 PTY tests via `expectrl`; stub `git`/`gh`/`codex` via temp `bin/` on `$PATH`
- Fixture configs: `tests/fixtures/configs/` (12 TOML files)
- Fixture repos: `tests/fixtures/repos/` (6 repo shapes for triage)
- `mock-discovery-responses/fully-equipped.json` — a pre-built `DiscoveryAssessment` JSON blob (not NDJSON)

### What's missing
- **No agent NDJSON fixture files** — adapters are tested with 2-line hardcoded strings only (`claude.rs:326`, `codex.rs:355`)
- **No rendering assertions** — `drawn_frames()` is captured but never substring-matched against expected TUI content
- **Triage flow never PTY-tested** — no `--triage-only` PTY test exists
- **Scroll keys untested** — `j`/`k` never sent; `WORKERS_VIEWPORT_OFFSET` never asserted
- **Operator hotkeys untested** — `r`/`l`/`p` keys have zero test coverage
- **Error paths untested** — `turn.failed`, non-zero exit codes, malformed NDJSON, missing envelope marker
- **Shutdown screen never verified** — content never checked in any test
- **Triage wizard never exercised** — `run_repo_health_wizard()` (`tui.rs:1632`) untouched

---

## Desired End State

```
tests/
  fixtures/
    agent-responses/
      claude/
        happy-path.jsonl          # success: message_start → content deltas → result/success
        multi-turn.jsonl           # success: multiple content_block_start cycles
        turn-failed.jsonl          # failure: result/subtype=error
        malformed-ndjson.jsonl     # mixed: some invalid lines + final result
      codex/
        happy-path.jsonl           # success: thread.started → turn.completed
        turn-failed.jsonl          # failure: turn.failed event
        error-event.jsonl          # failure: error event mid-stream
      discovery/
        claude-discovery.jsonl     # full NDJSON stream ending with <<GARDENER_JSON_START>>...<<GARDENER_JSON_END>>
        codex-discovery.jsonl      # same for codex
        no-envelope.jsonl          # stream with no envelope markers
        wrong-state.jsonl          # envelope with state != "seeding"

tests/
  phase05_agent_adapters.rs       # adapter parsing against fixtures
  phase06_tui_rendering.rs        # render_dashboard / draw_triage / draw_report / draw_shutdown
  phase07_hotkeys.rs              # all hotkeys in-process via FakeTerminal
  phase08_triage_integration.rs   # full run_triage() with fakes
  hotkey_pty_e2e.rs               # extended: screen-content asserts, triage PTY, operator keys
```

All existing tests continue to pass. The instrumentation linter (`tests/instrumentation_lint.rs`) passes.

---

## Key Discoveries

| Finding | Location |
|---|---|
| `render_dashboard()` returns a `String` of the ratatui buffer | `tui.rs:661` |
| `FakeTerminal::drawn_frames()` returns this string | `runtime/mod.rs:875` |
| `enqueue_keys()` feeds `poll_key()` with zero-timeout drain | `runtime/mod.rs:910` |
| `handle_hotkeys()` polls every 10 ms via `terminal.poll_key(10)` | `worker_pool.rs:338` |
| Scroll state lives in 4 thread-locals: `WORKERS_VIEWPORT_SELECTED` etc. | `tui.rs:1270` |
| `action_for_key_with_mode()` gates `r`/`l`/`p` behind `operator_hotkeys` bool | `hotkeys.rs:89` |
| `GARDENER_OPERATOR_HOTKEYS` env var enables operator keys | `hotkeys.rs:104` |
| Triage non-interactive guard checks `CLAUDECODE`, `CODEX_THREAD_ID`, `CI`, non-TTY | `triage.rs:149` |
| `run_repo_health_wizard()` opens its own `Terminal<CrosstermBackend>` | `tui.rs:1632` |
| `FakeTerminal::close_ui()` is a no-op — safe to call from triage tests | `runtime/mod.rs:920` |
| Discovery failure is swallowed; falls back to `DiscoveryAssessment::unknown()` | `triage.rs:228` |
| `parse_last_envelope` uses `rfind` — takes the LAST envelope in the stream | `output_envelope.rs:20` |
| Codex detection: checks for `turn.failed` first (forward), then `turn.completed` (reverse) | `codex.rs:213` |
| Agent NDJSON fixture format: one JSON object per line, `\n`-terminated | `claude.rs:158` / `codex.rs:168` |

---

## What We're NOT Doing

- **No vt100 backend** — Codex-RS uses `vt100::Parser` as a ratatui backend for pixel-level assertions. Gardener's `render_dashboard()` already returns a `String` buffer; that's sufficient.
- **No mock HTTP server** — Gardener spawns CLIs, not HTTP. Stub binaries on `$PATH` are the correct analog.
- **No wizard PTY test** — `run_repo_health_wizard()` opens its own alternate screen; testing it requires a separate PTY session. Deferred; existing fallback-to-defaults coverage is sufficient for now.
- **No live agent smoke tests** — no `#[ignore]` tests hitting real Claude/Codex. Those belong in a separate CI gate.
- **No concurrency stress tests** — log file corruption from parallel writes is a known issue but out of scope here.

---

## Implementation Approach

The four testing layers, in dependency order:

```
Layer 1: Fixture NDJSON files  (data, no code)
    ↓
Layer 2: Adapter unit tests    (FakeProcessRunner + fixtures)
    ↓
Layer 3: TUI rendering tests   (render_dashboard + AppState construction)
Layer 3: Hotkey in-process     (run_with_runtime + FakeTerminal + enqueue_keys)
Layer 3: Triage integration    (run_triage() + FakeProcessRunner + fixtures)
    ↓
Layer 4: PTY e2e enhancements  (expectrl + screen-content expects)
```

Layers 2/3 are independent and can be built in parallel. Layer 4 builds on the fixture
work from Layer 1.

---

## Phase 1 — Agent NDJSON Fixture Library

**Overview**: Create realistic fixture files sourced from real agent runs in this branch.
For each fixture, the raw stdout of the `claude`/`codex` CLI is captured and stored.
Where real captures aren't available, construct from the known protocol (documented in
`agent/protocol.rs:42-89`).

### How to source fixtures

The otel-logs (`otel-logs.jsonl`) don't capture raw agent stdout — they only log
`process.spawn`/`process.exit` with byte counts. To get real NDJSON:

```bash
# Capture claude output directly (run once, check into tests/fixtures/)
claude -p "echo hello" --output-format stream-json --verbose 2>/dev/null \
  > tests/fixtures/agent-responses/claude/happy-path.jsonl

# Or from existing worktree session logs if the branch has them
```

For cases where real captures don't exist, construct minimal valid NDJSON based on the
adapters' protocol parsing (`protocol.rs:42-89`).

### Files to create

**`tests/fixtures/agent-responses/claude/happy-path.jsonl`**
```jsonl
{"type":"message_start","message":{"id":"msg_01","model":"claude-opus-4-6","role":"assistant"}}
{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Working on it..."}}
{"type":"content_block_stop","index":0}
{"type":"tool_use","id":"tool_01","name":"bash","input":{"command":"echo hello"}}
{"type":"tool_result","tool_use_id":"tool_01","content":"hello\n"}
{"type":"result","subtype":"success","result":{"summary":"Ran echo hello","cost_usd":0.001}}
```

**`tests/fixtures/agent-responses/claude/turn-failed.jsonl`**
```jsonl
{"type":"message_start","message":{"id":"msg_02","model":"claude-opus-4-6","role":"assistant"}}
{"type":"result","subtype":"error","result":{"error":"context_length_exceeded"}}
```

**`tests/fixtures/agent-responses/claude/malformed-ndjson.jsonl`**
```jsonl
{"type":"message_start","message":{"id":"msg_03"}}
not valid json at all
{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}
another bad line
{"type":"result","subtype":"success","result":{"summary":"completed despite noise"}}
```

**`tests/fixtures/agent-responses/codex/happy-path.jsonl`**
```jsonl
{"type":"thread.started","thread_id":"thread_01"}
{"type":"turn.started","turn_id":"turn_01"}
{"type":"item.started","item":{"type":"function_call","name":"bash","id":"item_01"}}
{"type":"item.updated","item":{"type":"function_call","name":"bash","id":"item_01","output":"hello\n"}}
{"type":"item.completed","item":{"type":"function_call","name":"bash","id":"item_01"}}
{"type":"turn.completed","result":{"summary":"Ran bash command","output_file":"/tmp/out.json"}}
```

**`tests/fixtures/agent-responses/codex/turn-failed.jsonl`**
```jsonl
{"type":"thread.started","thread_id":"thread_02"}
{"type":"turn.started","turn_id":"turn_02"}
{"type":"turn.failed","reason":"sandbox_violation","message":"Command not permitted"}
```

**`tests/fixtures/agent-responses/codex/error-event.jsonl`**
```jsonl
{"type":"thread.started","thread_id":"thread_03"}
{"type":"error","reason":"rate_limit","message":"Too many requests"}
```

**`tests/fixtures/agent-responses/discovery/codex-discovery.jsonl`**
```jsonl
{"type":"thread.started","thread_id":"thread_04"}
{"type":"turn.started","turn_id":"turn_04"}
{"type":"turn.completed","result":{"summary":"Discovery complete"}}
<<GARDENER_JSON_START>>
{"schema_version":1,"state":"seeding","payload":{"gardener_output":{"agent_steering":{"grade":"B","summary":"CLAUDE.md present","issues":[],"strengths":["Well-structured steering"]},"knowledge_accessible":{"grade":"A","summary":"Good docs","issues":[],"strengths":["README comprehensive"]},"mechanical_guardrails":{"grade":"C","summary":"No linting","issues":["No pre-commit hooks"],"strengths":[]},"local_feedback_loop":{"grade":"B","summary":"Tests present","issues":[],"strengths":["cargo test works"]},"coverage_signal":{"grade":"C","summary":"Low coverage","issues":["No coverage tooling"],"strengths":[]},"overall_readiness_score":68,"overall_readiness_grade":"C","primary_gap":"mechanical_guardrails","notable_findings":"Solid foundation","scope_notes":""}}}
<<GARDENER_JSON_END>>
```

**`tests/fixtures/agent-responses/discovery/no-envelope.jsonl`**
```jsonl
{"type":"thread.started","thread_id":"thread_05"}
{"type":"turn.completed","result":{"summary":"Done but forgot to output envelope"}}
```

**`tests/fixtures/agent-responses/discovery/wrong-state.jsonl`**
```jsonl
{"type":"turn.completed","result":{}}
<<GARDENER_JSON_START>>
{"schema_version":1,"state":"reviewing","payload":{}}
<<GARDENER_JSON_END>>
```

### Success criteria
- All fixture files parse without errors when fed through the respective adapter
- `parse_last_envelope` accepts the discovery fixtures with valid envelopes
- `parse_last_envelope` returns appropriate errors for `no-envelope` and `wrong-state`

### Confirmation gate
Review fixture content against a real `claude`/`codex` run from this branch before proceeding.

---

## Phase 2 — Agent Adapter Tests

**Overview**: New test file `tests/phase05_agent_adapters.rs` that drives `ClaudeAdapter`
and `CodexAdapter` via `FakeProcessRunner` pre-loaded with Phase 1 fixtures.

### Changes required

**New file: `tests/phase05_agent_adapters.rs`**

```rust
// Pattern: load fixture, push as FakeProcessRunner response, call adapter, assert result

fn load_fixture(path: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent-responses")
            .join(path)
    ).unwrap()
}

#[test]
fn claude_happy_path_returns_success() {
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: load_fixture("claude/happy-path.jsonl"),
        stderr: String::new(),
    }));
    let result = run_claude_adapter(&process, "do a task", &config());
    assert!(matches!(result.terminal, AgentTerminal::Success));
    assert!(result.events.iter().any(|e| matches!(e.kind, AgentEventKind::ToolCall)));
}

#[test]
fn claude_turn_failed_returns_failure() { ... }

#[test]
fn claude_malformed_lines_are_skipped_not_fatal() { ... }

#[test]
fn codex_happy_path_returns_success() { ... }

#[test]
fn codex_turn_failed_detected_before_completed_scan() { ... }

#[test]
fn codex_error_event_returns_failure() { ... }

#[test]
fn discovery_valid_envelope_parsed() {
    // push codex-discovery.jsonl, call run_discovery(), assert DiscoveryAssessment fields
    let assessment = run_discovery_with_process(&process, ...);
    assert_eq!(assessment.overall_readiness_grade, "C");
    assert_eq!(assessment.primary_gap, "mechanical_guardrails");
}

#[test]
fn discovery_no_envelope_falls_back_to_unknown() { ... }

#[test]
fn discovery_wrong_state_falls_back_to_unknown() { ... }

#[test]
fn discovery_nonzero_exit_code_falls_back_to_unknown() { ... }
```

### Success criteria
- All 9+ tests pass
- `spawned()` assertions confirm correct binary name and args for each adapter

---

## Phase 3 — TUI Rendering Tests

**Overview**: New test file `tests/phase06_tui_rendering.rs` exercising every screen variant
via `render_dashboard()` / `FakeTerminal::drawn_frames()`. Assertions are substring-match
on rendered text — not pixel-perfect, stable across minor layout tweaks.

### Changes required

**New file: `tests/phase06_tui_rendering.rs`**

```rust
// Dashboard: queue stats header
#[test]
fn dashboard_header_shows_queue_stats() {
    let workers = vec![worker_row("w-01", "doing", "Fix the bug", ...)];
    let frame = render_dashboard(workers, stats(ready:2, active:1, failed:0, ...), ...);
    assert!(frame.contains("ready 2"));
    assert!(frame.contains("active 1"));
    assert!(frame.contains("GARDENER live queue"));
}

// Dashboard: all worker state colors via state string presence
#[test]
fn dashboard_worker_states_all_render() {
    for state in ["doing","reviewing","failed","complete","idle","planning","gitting"] {
        let frame = render_dashboard(vec![worker_row("w-01", state, "task", ...)], ...);
        assert!(frame.contains(state), "state {state} not in frame");
    }
}

// Dashboard: zombie/problems panel appears
#[test]
fn dashboard_problems_panel_on_zombie_worker() {
    let zombie = worker_row_with("w-01", lease_held: true, session_missing: true, ...);
    let frame = render_dashboard(vec![zombie], ...);
    assert!(frame.contains("Problems Requiring Human"));
}

// Dashboard: empty backlog
#[test]
fn dashboard_empty_backlog_shows_placeholder() {
    let frame = render_dashboard(vec![], stats_all_zero(), backlog_empty(), ...);
    assert!(frame.contains("No backlog items"));
}

// Dashboard: backlog P0/P1/P2 badges
#[test]
fn dashboard_backlog_priority_badges() {
    let frame = render_with_backlog(vec![
        backlog_item("P0", "Critical task"),
        backlog_item("P1", "Normal task"),
    ]);
    assert!(frame.contains("P0"));
    assert!(frame.contains("Critical task"));
}

// Triage screen
#[test]
fn triage_screen_shows_activity_feed() {
    let terminal = FakeTerminal::new(true);
    terminal.draw_triage(
        &["Scanning repository".to_string()],
        &["agent: codex".to_string()],
    ).unwrap();
    let frames = terminal.drawn_frames();
    assert!(frames[0].contains("GARDENER triage mode") || ... /* check draw_triage path */);
}

// Report screen
#[test]
fn report_screen_shows_file_content() {
    let terminal = FakeTerminal::new(true);
    terminal.draw_report("/tmp/quality.md", "grade: B\noverall: good").unwrap();
    let draws = terminal.report_draws();
    assert_eq!(draws[0].0, "/tmp/quality.md");
    assert!(draws[0].1.contains("grade: B"));
}

// Shutdown screen: normal vs error accent
#[test]
fn shutdown_screen_error_title_recorded() {
    let terminal = FakeTerminal::new(true);
    terminal.draw_shutdown_screen("error: disk full", "out of space").unwrap();
    let screens = terminal.shutdown_screens();
    assert_eq!(screens[0].0, "error: disk full");
    assert!(screens[0].1.contains("out of space"));
}

// Triage: stage progress detection
#[test]
fn triage_stage_progress_all_stages() {
    // triage_stage_progress() in tui.rs:323 — test stage detection from activity strings
    let stages = triage_stage_progress(&[
        "Scanning repository shape".to_string(),
        "Detecting tools".to_string(),
    ]);
    assert_eq!(stages[0].state, StageState::Done);
    assert_eq!(stages[1].state, StageState::Current);
    assert_eq!(stages[2].state, StageState::Future);
}
```

**Possible `FakeTerminal` extension** (if `draw_triage` content is not captured in `drawn_frames`):
- Capture triage draws in a new `triage_draws: Arc<Mutex<Vec<(Vec<String>, Vec<String>)>>>` field
- Add `triage_draws()` accessor
- `draw_triage(activity, artifacts)` appends to it

### Success criteria
- All rendering tests pass without a real terminal
- Each test asserts at least one content string from the rendered frame

---

## Phase 4 — Hotkey Integration Tests

**Overview**: New test file `tests/phase07_hotkeys.rs` that calls `run_with_runtime` with
`FakeTerminal::enqueue_keys()` to exercise all hotkey paths in-process.

The challenge: `handle_hotkeys()` is in the inner loop of `run_worker_pool_fsm()`. The
existing pattern (`phase1_contracts.rs:155`) calls `run_with_runtime` end-to-end, which
reaches `handle_hotkeys` naturally.

### Changes required

**New file: `tests/phase07_hotkeys.rs`**

```rust
// Helper: build runtime with pre-injected keys and N tasks
fn hotkey_runtime(keys: Vec<char>, task_count: usize) -> (ProductionRuntime, Arc<FakeTerminal>) {
    let terminal = Arc::new(FakeTerminal::new(true));
    terminal.enqueue_keys(keys);
    let process = FakeProcessRunner::default();
    // Push enough agent responses (test_mode bypasses agent for scheduler tests,
    // but for agent-path tests push real NDJSON fixtures)
    let runtime = ProductionRuntime {
        clock: Arc::new(ProductionClock),
        file_system: Arc::new(seed_fs_with_tasks(task_count)),
        process_runner: Arc::new(process),
        terminal: Arc::clone(&terminal),
    };
    (runtime, terminal)
}

// q quits before processing all tasks
#[test]
fn hotkey_q_interrupts_run() {
    let (runtime, terminal) = hotkey_runtime(vec!['q'], 100);
    // quit-after=100 but q causes early exit
    let result = gardener::run_with_runtime(&["--quit-after", "100", ...], ...);
    // dashboard was drawn at least once before quit
    assert!(terminal.dashboard_draw_count() > 0);
    // but not 100 times (not all tasks processed)
    // check BacklogStore has remaining tasks
}

// v shows report, b returns to dashboard
#[test]
fn hotkey_v_shows_report_b_returns() {
    let (runtime, terminal) = hotkey_runtime(vec!['v', 'b', 'q'], 1);
    gardener::run_with_runtime(...);
    assert!(!terminal.report_draws().is_empty(), "v should trigger report draw");
    // dashboard redrawn after b
    assert!(terminal.dashboard_draw_count() >= 2);
}

// g regenerates quality report
#[test]
fn hotkey_g_regenerates_report() {
    let (runtime, terminal) = hotkey_runtime(vec!['g', 'q'], 1);
    // seed a quality.md with OLD_MARKER
    runtime.file_system.write_string(quality_path, "OLD_MARKER").unwrap();
    // push git process responses + agent response for quality regeneration
    // ...
    gardener::run_with_runtime(...);
    let new_content = runtime.file_system.read_to_string(quality_path).unwrap();
    assert!(!new_content.contains("OLD_MARKER"));
}

// j/k scroll — assert viewport offset changes
#[test]
fn hotkey_j_k_scroll_workers() {
    // needs 10+ workers to enable scrolling
    let (runtime, terminal) = hotkey_runtime(vec!['j', 'j', 'k', 'q'], 10);
    gardener::run_with_runtime(&["--parallelism", "10", ...], ...);
    // drawn_frames should show different viewport windows
    let frames = terminal.drawn_frames();
    // frame after jj should show "Workers (03-N/M)" or similar
    assert!(frames.iter().any(|f| f.contains("Workers (0")));
}

// operator hotkeys: r releases leases
#[test]
fn hotkey_r_retries_stale_leases() {
    let (runtime, terminal) = hotkey_runtime(vec!['r', 'q'], 1);
    // run with GARDENER_OPERATOR_HOTKEYS=1
    run_with_env(&[("GARDENER_OPERATOR_HOTKEYS", "1")], ...);
    assert!(terminal.written_lines().iter().any(|l| l.contains("released")));
}

// operator hotkeys: p escalates to P0
#[test]
fn hotkey_p_escalates_to_p0() {
    let (runtime, terminal) = hotkey_runtime(vec!['p', 'q'], 1);
    run_with_env(&[("GARDENER_OPERATOR_HOTKEYS", "1")], ...);
    assert!(terminal.written_lines().iter().any(|l| l.contains("P0")));
}

// operator hotkeys gated: r is no-op without env var
#[test]
fn operator_hotkeys_gated_without_env_var() {
    let (runtime, terminal) = hotkey_runtime(vec!['r', 'q'], 1);
    // no GARDENER_OPERATOR_HOTKEYS
    run_with_env(&[], ...);
    // no "released" message written
    assert!(!terminal.written_lines().iter().any(|l| l.contains("released")));
}
```

### Success criteria
- All 7 hotkey tests pass
- Each test asserts on observable side-effects (draw count, written lines, file content, or store state)

---

## Phase 5 — Triage Flow Integration Tests

**Overview**: New test file `tests/phase08_triage_integration.rs`. Tests `run_triage()`
directly (library call, no PTY) using `FakeProcessRunner` + fixture NDJSON. Covers the
triage decision logic, discovery parsing, and error paths.

Key setup: triage requires `FakeTerminal::stdin_is_tty() = false` OR a TTY with the
non-interactive guard bypassed. Since `run_triage` checks `is_non_interactive()` at
`triage.rs:149`, we either set `is_tty = false` (non-interactive path) for most tests,
or override the env guard for the interactive-path tests.

### Changes required

**New file: `tests/phase08_triage_integration.rs`**

```rust
// --- Triage decision tests ---

#[test]
fn triage_not_needed_when_sha_matches() {
    // profile exists with head_sha == current HEAD
    // assert run_with_runtime exits without calling discovery agent
    let process = FakeProcessRunner::default();
    // push git rev-parse HEAD response matching profile sha
    process.push_response(git_rev_parse("abc123"));
    run_with_runtime_no_triage(&process, "abc123"); // profile sha = abc123
    assert!(process.spawned().iter().all(|r| r.program != "codex" && r.program != "claude"));
}

#[test]
fn triage_needed_when_profile_missing() { ... }

#[test]
fn triage_needed_when_sha_differs() { ... }

#[test]
fn triage_needed_when_commits_exceed_stale_threshold() { ... }

#[test]
fn force_retriage_overrides_sha_match() { ... }

// --- Discovery parsing tests ---

#[test]
fn discovery_success_populates_profile() {
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: load_fixture("discovery/codex-discovery.jsonl"),
        stderr: String::new(),
    }));
    let profile = run_triage_non_interactive(&process, ...);
    assert_eq!(profile.readiness_grade, "C");
    assert_eq!(profile.primary_gap, "mechanical_guardrails");
}

#[test]
fn discovery_failure_falls_back_to_unknown_grade() {
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: "agent crashed".to_string(),
    }));
    let profile = run_triage_non_interactive(&process, ...);
    assert_eq!(profile.readiness_grade, "F"); // DiscoveryAssessment::unknown()
}

#[test]
fn discovery_no_envelope_falls_back_to_unknown() {
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: load_fixture("discovery/no-envelope.jsonl"),
        stderr: String::new(),
    }));
    let profile = run_triage_non_interactive(&process, ...);
    assert_eq!(profile.readiness_grade, "F");
}

// --- Non-interactive guard ---

#[test]
fn triage_blocked_in_ci_environment() {
    let terminal = FakeTerminal::new(true); // tty=true but CI env var set
    let result = run_triage_with_env(&[("CI", "1")], &terminal, ...);
    assert!(matches!(result, Err(GardenerError::Cli(_))));
    assert!(terminal.written_lines().iter().any(|l| l.contains("non-interactive")));
}

#[test]
fn triage_non_interactive_path_writes_dimension_summaries() {
    // tty=false → non-interactive path in run_interview (triage_interview.rs:41)
    let terminal = FakeTerminal::new(false);
    run_triage_no_tty(&terminal, ...);
    let lines = terminal.written_lines();
    assert!(lines.iter().any(|l| l.contains("agent_steering")));
}

// --- Triage activity feed UI ---

#[test]
fn triage_draw_called_on_each_phase() {
    let terminal = FakeTerminal::new(true);
    run_triage_full_non_interactive(&terminal, ...);
    // draw_triage is called from push_triage_update which is called 6+ times
    assert!(terminal.dashboard_draw_count() > 0 || /* triage draw count */ > 0);
}

// --- Profile written to disk ---

#[test]
fn triage_writes_profile_file() {
    let fs = FakeFileSystem::default();
    run_triage_with_fs(&fs, ...);
    assert!(fs.exists(Path::new("/.gardener/repo-intelligence.toml")));
    let content = fs.read_to_string(...).unwrap();
    assert!(content.contains("schema_version"));
}
```

**Possible `FakeTerminal` extension**: add `triage_draw_count()` accessor if the triage
`draw_triage()` call isn't captured by the existing `dashboard_draw_count()`.

### Success criteria
- All triage decision tests pass (5 tests)
- Discovery parsing tests pass for all envelope scenarios (4 tests)
- Non-interactive guard correctly blocks triage (2 tests)
- Profile written to FakeFileSystem after successful triage

---

## Phase 6 — PTY E2E Enhancements

**Overview**: Extend `hotkey_pty_e2e.rs` with screen-content verification and new scenarios.
Use `expectrl::Session::expect()` to assert on rendered terminal output before/after each
key press.

### Changes required

**In `tests/hotkey_pty_e2e.rs`:**

#### Enhancement 1: Screen content verification for existing tests

```rust
// Current test sends v/g/b/q without asserting screen content.
// Enhanced version:
#[test]
fn pty_e2e_hotkeys_v_g_b_q_with_screen_content_verification() {
    let (cmd, dir) = setup_pty_fixture();
    let mut session = Session::spawn(cmd).unwrap();

    // Wait for dashboard to appear before sending keys
    session.expect("GARDENER live queue").unwrap();

    // v → report screen
    session.send("v").unwrap();
    session.expect("Quality report view").unwrap();

    // g → regenerate (still on report screen)
    session.send("g").unwrap();
    // report content changes (OLD_MARKER gone)

    // b → back to dashboard
    session.send("b").unwrap();
    session.expect("GARDENER live queue").unwrap();

    // q → quit
    session.send("q").unwrap();
    session.expect(Eof).unwrap();

    // side-effect assertions (existing)
    assert!(!fs::read_to_string(dir.path().join(".gardener/quality.md"))
        .unwrap().contains("OLD_MARKER"));
}
```

#### Enhancement 2: Scroll key verification

```rust
#[test]
fn pty_e2e_j_k_scroll_changes_viewport() {
    let (cmd, dir) = setup_pty_fixture(); // has 500 tasks → many workers
    let mut session = Session::spawn(cmd).unwrap();
    session.expect("GARDENER live queue").unwrap();

    // scroll down twice
    session.send("jj").unwrap();
    // header changes to show offset: "Workers (03-N/M)"
    session.expect(Regex::new(r"Workers \(\d\d-").unwrap()).unwrap();

    session.send("q").unwrap();
    session.expect(Eof).unwrap();
}
```

#### Enhancement 3: Operator hotkeys PTY test

```rust
#[test]
fn pty_e2e_operator_hotkeys_r_l_p() {
    let (mut cmd, dir) = setup_pty_fixture();
    cmd.env("GARDENER_OPERATOR_HOTKEYS", "1");
    let mut session = Session::spawn(cmd).unwrap();
    session.expect("GARDENER live queue").unwrap();

    // r → retry stale leases
    session.send("r").unwrap();
    session.expect("released").unwrap(); // structured_fallback_line or drawn frame

    // l → force release
    session.send("l").unwrap();
    session.expect("released").unwrap();

    // p → escalate to P0
    session.send("p").unwrap();
    session.expect("P0").unwrap();

    session.send("q").unwrap();
    session.expect(Eof).unwrap();
}
```

#### Enhancement 4: Triage PTY test (`--triage-only`)

```rust
fn setup_triage_pty_fixture() -> (Command, TempDir) {
    let dir = TempDir::new().unwrap();
    // No profile file → triage_needed() returns Needed
    // stub codex that reads fixture and streams codex-discovery.jsonl to stdout
    write_exec(&dir, "codex", &format!(
        r#"#!/bin/sh
cat "{}/tests/fixtures/agent-responses/discovery/codex-discovery.jsonl"
"#,
        env!("CARGO_MANIFEST_DIR")
    ));
    // stub git: rev-parse HEAD returns new sha
    write_exec(&dir, "git", "...");
    // config: triage.stale_after_commits = 0 (always retriage)
    // ...
    (gardener_cmd_with_triage_only(&dir), dir)
}

#[test]
fn pty_e2e_triage_only_shows_triage_screen_and_exits() {
    let (cmd, dir) = setup_triage_pty_fixture();
    let mut session = Session::spawn(cmd).unwrap();
    session.expect("GARDENER triage mode").unwrap();
    session.expect("Scanning").unwrap(); // activity feed entry
    session.expect(Eof).unwrap();
    // profile written
    assert!(dir.path().join(".gardener/repo-intelligence.toml").exists());
}
```

#### Enhancement 5: Shutdown screen

```rust
#[test]
fn pty_e2e_shutdown_screen_on_quit() {
    // In normal operation, q shows a shutdown screen before exiting
    let (cmd, _dir) = setup_pty_fixture();
    let mut session = Session::spawn(cmd).unwrap();
    session.expect("GARDENER live queue").unwrap();
    session.send("q").unwrap();
    // Shutdown screen should appear momentarily
    // (only if gardener renders one on quit — verify by checking tui.rs shutdown call site)
    session.expect(Eof).unwrap();
}
```

### Success criteria
- All 5 enhanced/new PTY tests pass
- `session.expect()` calls verify actual terminal content, not just side effects
- Triage PTY test writes a valid profile to disk

---

## Testing Strategy

**Unit level** (Phases 1-2): Adapter fixture tests run fast (<100ms each), no filesystem, no PTY.

**Integration level** (Phases 3-5): In-process `run_with_runtime` calls with fakes. Some
tests need `serial_test` if they touch thread-locals (`WORKERS_VIEWPORT_OFFSET` etc.).
Add `serial_test = "3.2.0"` to dev-dependencies if not already present.

**E2E level** (Phase 6): PTY tests are inherently slower (spawn real binary, wait for output).
Keep `--test-threads=1` or use `#[serial]` for PTY tests since they share global state
(PATH, process spawning).

**CI**: All phases run in standard `cargo nextest run`. No new env vars needed except
`GARDENER_OPERATOR_HOTKEYS=1` for operator hotkey tests (set inline per test via `.env()`
on `Command`).

**Instrumentation linter**: Any new functions added to src/ that have side effects must
include `append_run_log(` or `structured_fallback_line(`. Add `FakeTerminal::triage_draws()`
to the exclusion list in `instrumentation_lint.rs` if it's added as a pure accessor.

---

## References

- `tools/gardener/src/tui.rs` — `render_dashboard()` at :661, `draw_dashboard_frame()` at :743, triage at :1128, report at :1245, shutdown at :1429, wizard at :1632
- `tools/gardener/src/runtime/mod.rs` — `FakeTerminal` at :828, `FakeProcessRunner` at :934, `handle_hotkeys()` dispatch in `worker_pool.rs` at :324
- `tools/gardener/src/hotkeys.rs` — all bindings and `action_for_key_with_mode()` at :89
- `tools/gardener/src/triage.rs` — `triage_needed()` at :56, `run_triage()` at :123, non-interactive guard at :149
- `tools/gardener/src/agent/claude.rs` / `codex.rs` — NDJSON parsing, terminal result detection
- `tools/gardener/src/output_envelope.rs` — `parse_last_envelope()` at :15
- `tools/gardener/tests/hotkey_pty_e2e.rs` — existing PTY fixture setup, keystroke delivery patterns
- `tools/gardener/tests/phase1_contracts.rs` — canonical pattern for assembling `ProductionRuntime` from fakes
- `codex-rs/codex-rs/utils/pty/` — reference implementation for PTY cursor-query injection (not needed here, `expectrl` handles it)
