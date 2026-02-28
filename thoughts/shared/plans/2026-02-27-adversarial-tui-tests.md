# Adversarial TUI Integration Tests — Bug-Hunting Round

## Overview

The prior round (phases 5–8) confirmed existing behavior with synthetic fixtures. Every test
passed on the first run because the fixtures were hand-crafted to match what the code expected.
This round actively tries to break things: probing edge cases at integration seams, exercising
error paths that are currently swallowed, and validating behavior with malformed/unexpected inputs.

## Current State — What Research Found

### Confirmed Bugs

| # | Bug | Location | Severity |
|---|---|---|---|
| B1 | `current_head_sha` failure writes `"unknown"` to profile; next run `"unknown"=="unknown"` → triage never re-triggers | `triage.rs:77,307-308` | High |
| B2 | `commits_since_profile_head` error swallowed → defaults to 0 → stale profile kept as "not stale" | `triage.rs:95`, `repo_intelligence.rs:176` | Medium |
| B3 | `AGENTS.md` triggers `DetectedAgent::Both` → silently defaults to Codex via wildcard `_` match | `triage.rs:172-175` | Low |
| B4 | `'p'` (ParkEscalate) creates P0 escalation task even with 0 active workers — no guard | `worker_pool.rs:413-438` | Low |
| B5 | Profile TOML `schema_version` is never validated — a v2 profile is silently accepted | `repo_intelligence.rs:102-104` | Medium |
| B6 | Discovery result lost if `run_interview` fails after successful discovery | `triage.rs:274-279` | Medium |
| B7 | `write_profile` failure after completed triage loses entire session | `triage.rs:325` | Medium |
| B8 | Any JSON object with `"type":"error"` terminates a Codex turn as failure, even if informational | `codex.rs:213-215` | Medium |
| B9 | Claude `content_block_start` maps to `TurnStarted` — multiple content blocks produce multiple `TurnStarted` events within one turn | `protocol.rs:68` | Low |

### Swallowed Error Paths (Not Bugs Per Se, But Testable Behavior)

| # | Swallowed Error | Location |
|---|---|---|
| S1 | `current_head_sha` error → `"unknown"` | `triage.rs:77` |
| S2 | `commits_since_profile_head` error → `0` | `triage.rs:95` |
| S3 | `run_discovery` all errors → `DiscoveryAssessment::unknown()` | `triage.rs:228-237` |
| S4 | `commits_since_profile_head` non-zero git exit → `Ok(0)` | `repo_intelligence.rs:176` |

### Test Infrastructure Gaps

| # | Gap | Impact |
|---|---|---|
| G1 | `FakeProcessRunner::wait()` ignores handle parameter — returns responses in push order | Could mask out-of-order wait bugs |
| G2 | `FakeTerminal::draw_shutdown_screen` doesn't call `draw()` — `drawn_frames()` never captures shutdown content | Shutdown rendering assertions would fail in surprising ways |
| G3 | `FakeFileSystem::exists()` doesn't check directories (only files) | Tests that check directory existence get wrong results |
| G4 | `FakeFileSystem::remove_file` never fails on missing paths | Cannot test ENOENT error handling |
| G5 | `FakeTerminal` ignores `heartbeat_interval_seconds` / `lease_timeout_seconds` | Zombie/stalled classification thresholds untestable via fakes |

---

## Desired End State

```
tests/
  phase09_adapter_edge_cases.rs      # adversarial adapter inputs
  phase10_triage_error_paths.rs      # triage integration seam bugs
  phase11_hotkey_edge_cases.rs       # hotkey edge cases + escalation guard
  phase12_render_edge_cases.rs       # render boundary conditions
```

Each test file probes a known bug or edge case. Tests that expose actual bugs (B1–B9) should
be written as `#[test] #[should_panic]` or assert the current (broken) behavior with a
`// BUG:` comment explaining the correct behavior. This documents bugs even before they're fixed.

---

## Key Discoveries

| Finding | Location |
|---|---|
| Claude adapter: missing `subtype` on `result` event → treated as Failure | `claude.rs:210-218` |
| Claude adapter: `.rev().find()` means last `result` event wins | `claude.rs:201-204` |
| Codex adapter: forward `.find()` for `turn.failed` means first failure wins | `codex.rs:213-215` |
| Codex adapter: `"error"` event type maps to same `TurnFailed` as `turn.failed` | `protocol.rs:51` |
| `map_claude_event`: `content_block_start` → `TurnStarted` (not turn boundary) | `protocol.rs:68` |
| `map_claude_event`: `message_stop`, `message_delta`, `ping` → `Unknown` | `protocol.rs:63-89` |
| `parse_last_envelope` uses `rfind` — last envelope wins if multiple exist | `output_envelope.rs:20` |
| Profile TOML deserialized with no `schema_version` check | `repo_intelligence.rs:102-104` |
| Discovery assessment: all fields required by serde (no `#[serde(default)]`) | `triage_discovery.rs:28-40` |
| `ensure_profile_for_run` error path trace: discovery errors swallowed, interview errors propagated | See error path trace below |
| `FakeProcessRunner::wait` dequeues by push-order, ignoring handle | `runtime/mod.rs:970-979` |
| `worker_pool.rs:50`: `parallelism.max(1)` is a redundant floor — `validate_config` already rejects 0 | `worker_pool.rs:50`, `config.rs:712-716` |
| `--quit-after 0` exits immediately with "Completed 0 of 0 task(s)" shutdown screen | `worker_pool.rs:69,296-319` |
| `draw_boot_stage` with non-TTY → early return, no dashboard draw | `lib.rs:495-497` |
| `execute_task_live` (the real worker FSM) is completely untested | `worker.rs:96-519` |

### Error Propagation Map for `ensure_profile_for_run`

```
ensure_profile_for_run
├─ triage_needed()?
│  ├─ read_profile()?                    → PROPAGATED (corrupt TOML aborts run)
│  ├─ current_head_sha()                 → SWALLOWED → "unknown"
│  └─ commits_since_profile_head()       → SWALLOWED → 0
├─ [NotNeeded] read_profile()?           → PROPAGATED
└─ [Needed] run_triage()?
   ├─ is_non_interactive?               → PROPAGATED (Cli error)
   ├─ run_discovery()                    → SWALLOWED → DiscoveryAssessment::unknown()
   ├─ run_interview()?                   → PROPAGATED (discovery result LOST)
   ├─ build_profile()                    → infallible
   ├─ current_head_sha()                → SWALLOWED → "unknown"
   └─ write_profile()?                   → PROPAGATED (triage session LOST)
```

---

## What We're NOT Doing

- **No live agent smoke tests** — no `#[ignore]` tests hitting real Claude/Codex
- **No `execute_task_live` tests** — the real worker FSM requires a full adapter stack; proper testing
  requires a larger test harness than this round's scope
- **No TOCTOU tests** — the profile race condition between concurrent triage runs requires
  multi-process coordination
- **No FakeProcessRunner handle-order fix** — documenting the gap, not fixing the test infrastructure
- **No PTY tests in this round** — Phase 6 from the prior plan covers those

---

## Implementation Approach

```
Phase 1: Adapter adversarial inputs     (FakeProcessRunner + edge-case NDJSON)
    ↓
Phase 2: Triage seam bug probes         (FakeFileSystem + FakeProcessRunner + error injection)
    ↓
Phase 3: Hotkey + render edge cases     (FakeTerminal + boundary inputs)
```

All three phases are independent — no dependencies between them.

---

## Phase 1 — Adapter Adversarial Inputs

**Overview**: New test file `tests/phase09_adapter_edge_cases.rs`. Each test constructs a
pathological NDJSON stream and feeds it through the real adapter, asserting on the actual
(possibly buggy) behavior.

### Test cases

**Claude adapter:**

```rust
#[test]
fn claude_result_without_subtype_is_failure() {
    // B: result event with no subtype field
    // Expected (current behavior): treated as Failure because "" != "success"
    // This is arguably correct but worth documenting
    let ndjson = r#"{"type":"message_start","message":{"id":"msg_01"}}
{"type":"result","result":{"summary":"done"}}
"#;
    // push, execute, assert terminal == Failure
}

#[test]
fn claude_result_with_unknown_subtype_is_failure() {
    // result event with subtype "partial" (neither "success" nor "error")
    let ndjson = r#"{"type":"message_start","message":{"id":"msg_01"}}
{"type":"result","subtype":"partial","result":{"summary":"partial result"}}
"#;
    // assert terminal == Failure
}

#[test]
fn claude_multiple_result_events_last_wins() {
    // First result is failure, second is success — last should win
    let ndjson = r#"{"type":"message_start","message":{"id":"msg_01"}}
{"type":"result","subtype":"error","result":{"error":"rate_limit"}}
{"type":"result","subtype":"success","result":{"summary":"retry worked"}}
"#;
    // assert terminal == Success (last result wins via .rev().find())
}

#[test]
fn claude_empty_stdout_exit_zero_is_error() {
    // Empty stdout, exit code 0 — no terminal event
    // push response with empty stdout, exit 0
    // assert Err containing "missing terminal result event"
}

#[test]
fn claude_result_without_result_field_has_null_payload() {
    // result event with subtype "success" but no "result" key
    let ndjson = r#"{"type":"result","subtype":"success"}
"#;
    // assert terminal == Success, payload == Value::Null
}

#[test]
fn claude_stderr_does_not_affect_success() {
    // Valid success result but stderr has warnings
    // push response with stderr = "WARNING: rate limit approaching"
    // assert terminal == Success
    // assert diagnostics contains the stderr warning
}

#[test]
fn claude_nonzero_exit_without_result_event_is_process_error() {
    // Events exist but no result event, exit code 1
    let ndjson = r#"{"type":"message_start","message":{"id":"msg_01"}}
{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"working"}}
"#;
    // push with exit_code: 1, stderr: "killed by OOM"
    // assert Err containing "killed by OOM"
}

#[test]
fn claude_multi_content_block_produces_multiple_turn_started_events() {
    // BUG B9: content_block_start maps to TurnStarted
    // Two content blocks in one turn should probably not produce two TurnStarted events
    let ndjson = r#"{"type":"message_start","message":{"id":"msg_01"}}
{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
{"type":"content_block_stop","index":0}
{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}
{"type":"content_block_stop","index":1}
{"type":"result","subtype":"success","result":{}}
"#;
    // assert events has TWO TurnStarted events (documenting bug B9)
    // assert terminal == Success
}
```

**Codex adapter:**

```rust
#[test]
fn codex_both_failed_and_completed_failure_wins() {
    // turn.failed appears BEFORE turn.completed — failure should win
    let ndjson = r#"{"type":"thread.started","thread_id":"t1"}
{"type":"turn.failed","reason":"sandbox_violation","message":"blocked"}
{"type":"turn.completed","result":{"summary":"somehow completed"}}
"#;
    // assert terminal == Failure (forward scan for turn.failed finds it first)
}

#[test]
fn codex_completed_before_failed_failure_still_wins() {
    // turn.completed appears FIRST but turn.failed appears later — failure still wins
    let ndjson = r#"{"type":"thread.started","thread_id":"t1"}
{"type":"turn.completed","result":{"summary":"completed first"}}
{"type":"turn.failed","reason":"late_failure","message":"failed after"}
"#;
    // assert terminal == Failure (forward scan finds turn.failed regardless of order)
}

#[test]
fn codex_error_event_is_treated_as_turn_failed() {
    // BUG B8: any {"type":"error"} terminates as failure
    // This could be an informational error that doesn't end the turn
    let ndjson = r#"{"type":"thread.started","thread_id":"t1"}
{"type":"error","reason":"rate_limit_warning","message":"approaching limit"}
{"type":"turn.completed","result":{"summary":"completed fine"}}
"#;
    // assert terminal == Failure (BUG: error event preempts turn.completed)
    // The turn.completed after the error is unreachable
}

#[test]
fn codex_empty_stdout_exit_zero_is_error() {
    // Empty stdout, exit 0 — no terminal event
    // assert Err containing "missing turn.completed or turn.failed event"
}

#[test]
fn codex_multiple_turn_completed_last_wins() {
    // Multiple turn.completed events — last should win via .rev().find()
    let ndjson = r#"{"type":"thread.started","thread_id":"t1"}
{"type":"turn.completed","result":{"summary":"first"}}
{"type":"turn.completed","result":{"summary":"second"}}
"#;
    // assert terminal == Success
    // assert payload["summary"] == "second"
}
```

**Discovery edge cases:**

```rust
#[test]
fn discovery_envelope_with_valid_markers_but_invalid_json() {
    let stdout = "<<GARDENER_JSON_START>>\nnot valid json\n<<GARDENER_JSON_END>>\n";
    // assert parse_last_envelope returns Err containing "invalid json"
}

#[test]
fn discovery_envelope_with_missing_gardener_output_field() {
    let stdout = r#"<<GARDENER_JSON_START>>
{"schema_version":1,"state":"seeding","payload":{"wrong_key":"data"}}
<<GARDENER_JSON_END>>
"#;
    // parse_last_envelope succeeds (it only checks schema_version and state)
    // but serde_json::from_value::<DiscoveryEnvelope> fails
    // assert run_discovery returns Err (which triage swallows → unknown)
}

#[test]
fn discovery_multiple_envelopes_last_wins() {
    // Two envelopes in stream — rfind takes the last one
    let stdout = r#"<<GARDENER_JSON_START>>
{"schema_version":1,"state":"seeding","payload":{"gardener_output":{...first...}}}
<<GARDENER_JSON_END>>
<<GARDENER_JSON_START>>
{"schema_version":1,"state":"seeding","payload":{"gardener_output":{...second...}}}
<<GARDENER_JSON_END>>
"#;
    // assert the second envelope's data is returned
}

#[test]
fn discovery_schema_version_2_rejected() {
    let stdout = r#"<<GARDENER_JSON_START>>
{"schema_version":2,"state":"seeding","payload":{}}
<<GARDENER_JSON_END>>
"#;
    // assert parse_last_envelope returns Err containing "schema_version must be 1"
}

#[test]
fn discovery_end_marker_before_start_marker_rejected() {
    let stdout = "<<GARDENER_JSON_END>>\n<<GARDENER_JSON_START>>\n{}\n";
    // assert parse_last_envelope returns Err
}
```

### Success criteria
- All tests compile and pass
- Tests documenting bugs (B8, B9) have `// BUG:` comments explaining incorrect behavior
- Tests confirming correct edge case handling have explanatory comments

---

## Phase 2 — Triage Integration Seam Bug Probes

**Overview**: New test file `tests/phase10_triage_error_paths.rs`. Tests exercise the error
propagation/swallowing paths identified in the error path trace above.

### Test cases

```rust
// --- B1: "unknown" SHA prevents future triage ---

#[test]
fn triage_needed_unknown_sha_profile_is_never_retriggered() {
    // BUG B1: Profile has head_sha = "unknown"
    // current_head_sha also fails → "unknown"
    // "unknown" == "unknown" → NotNeeded
    // The stale profile is used forever
    //
    // Seed a profile with head_sha = "unknown"
    // Push a failing git rev-parse response
    // Assert triage_needed returns NotNeeded
    // This documents the bug: a git failure permanently prevents retriage
}

// --- S1/S2: Swallowed errors in triage_needed ---

#[test]
fn triage_needed_git_revparse_failure_uses_unknown_sha() {
    // Seed profile with head_sha = "abc123"
    // Push a git rev-parse response with exit_code=1
    // current_head_sha fails → "unknown"
    // "abc123" != "unknown" → falls through to commits_since check
    // Push a git rev-list response with exit_code=1
    // commits_since_profile_head fails → unwrap_or(0) → 0
    // 0 > stale_after_commits (50) is false → NotNeeded
    //
    // This documents S1+S2: git failures cascade to NotNeeded
    // when they should arguably trigger Needed (conservative)
}

#[test]
fn triage_needed_commits_since_failure_defaults_not_stale() {
    // Seed profile with head_sha = "abc123"
    // Push git rev-parse returning "def456" (different SHA)
    // Push git rev-list returning exit_code=128 (git error)
    // commits_since_profile_head returns Ok(0) (inner swallow)
    // 0 <= stale_after_commits → NotNeeded
    //
    // Correct behavior: uncertain should default to Needed
}

// --- B5: No schema_version validation on profile TOML ---

#[test]
fn profile_with_schema_version_99_is_silently_accepted() {
    // BUG B5: read_profile has no schema_version check
    // Modify a valid profile TOML to have schema_version = 99
    // Assert read_profile succeeds (no error)
    // This documents the bug: old/future profiles are silently loaded
}

// --- S3: run_discovery errors swallowed ---

#[test]
fn run_discovery_process_error_falls_back_to_unknown() {
    // Push a response that makes process_runner.run() return Err
    // Assert that when triage swallows this, DiscoveryAssessment::unknown() is used
    // Specifically: grade == "F", primary_gap == "agent_steering"
}

#[test]
fn run_discovery_invalid_envelope_json_falls_back_to_unknown() {
    // Push a response with valid markers but invalid JSON
    // Assert fallback to unknown assessment
}

#[test]
fn run_discovery_missing_gardener_output_falls_back_to_unknown() {
    // Push a valid envelope but payload is {"wrong_key": ...}
    // serde_json::from_value::<DiscoveryEnvelope> fails
    // Assert fallback to unknown assessment
}

// --- B3: AGENTS.md → Both → Codex ---

#[test]
fn detect_agent_both_signals_defaults_to_codex() {
    // BUG B3: Create filesystem with AGENTS.md (triggers both signals)
    // No other agent-specific files
    // Assert detect_agent returns DetectedAgent::Both
    // Assert the wildcard match maps this to AgentKind::Codex
}

// --- B6/B7: Error propagation losing work ---

#[test]
fn discovery_lost_when_interview_write_line_fails() {
    // BUG B6: Set up a FakeTerminal that is_tty=false
    // AND inject a terminal write failure (FailingTerminal from phase1_contracts.rs)
    // Discovery succeeds but run_interview's write_line fails
    // Assert run_triage returns Err
    // Assert no profile was written to disk (discovery work lost)
}

// --- B1 deep: unknown SHA written to profile and persisted ---

#[test]
fn unknown_sha_written_to_profile_after_git_failure() {
    // Simulate run_triage where current_head_sha fails
    // After triage completes, read the written profile
    // Assert profile.meta.head_sha == "unknown"
    // Then assert that next call to triage_needed with this profile returns NotNeeded
    // (because "unknown" == "unknown")
}
```

### Success criteria
- All tests compile and pass
- Each bug test has a `// BUG Bn:` comment explaining the incorrect behavior
- Each swallowed-error test has a `// SWALLOWED:` comment explaining what was lost
- Tests use real module functions (`triage_needed`, `read_profile`, `run_discovery`, `parse_last_envelope`)

---

## Phase 3 — Hotkey & Render Edge Cases

**Overview**: New test files `tests/phase11_hotkey_edge_cases.rs` and
`tests/phase12_render_edge_cases.rs`.

### phase11_hotkey_edge_cases.rs

```rust
// --- B4: Escalation with zero active workers ---

#[test]
fn escalate_creates_task_even_with_zero_active_workers() {
    // BUG B4: ParkEscalate has no active-worker guard
    // Set up a scenario with workers all in "idle" state
    // Trigger ParkEscalate hotkey
    // Assert a P0 task is created titled "Escalation requested for 0 active worker(s)"
    // This documents the bug: meaningless escalation task created
}

// --- Hotkey edge cases ---

#[test]
fn unknown_events_between_known_events_dont_corrupt_state() {
    // Send event with type "future.unknown.event" between normal events
    // Assert it maps to AgentEventKind::Unknown
    // Assert it doesn't affect the terminal classification
}

#[test]
fn quit_after_zero_shows_shutdown_screen() {
    // Not a bug, but surprising behavior worth documenting
    // run_with_runtime with --quit-after 0, test_mode=true
    // Assert it returns Ok(0) without processing any tasks
    // Assert shutdown screen says "Completed 0 of 0 task(s)"
}

#[test]
fn operator_hotkeys_env_var_truthy_variants() {
    // Test all truthy variants: "1", "true", "yes", "TRUE", "Yes", " 1 "
    // Test falsy variants: "0", "false", "no", "", "maybe"
    // Note: operator_hotkeys_enabled reads GARDENER_OPERATOR_HOTKEYS env var
    // These test action_for_key_with_mode with the boolean directly
    // but we should also test the env var parsing
}
```

### phase12_render_edge_cases.rs

```rust
// --- Render boundary conditions ---

#[test]
fn render_dashboard_zero_width_zero_height() {
    // Should not panic, returns empty string
    let frame = render_dashboard(&[], &zero_stats(), &empty_backlog(), 0, 0);
    assert_eq!(frame, "");
}

#[test]
fn render_dashboard_width_1_height_1() {
    // Minimal non-zero dimensions — should not panic
    let frame = render_dashboard(
        &[make_worker("w-01", "doing", "task")],
        &zero_stats(), &empty_backlog(), 1, 1,
    );
    assert!(!frame.is_empty());
}

#[test]
fn render_dashboard_many_workers_small_viewport() {
    // 50 workers in a 120x10 viewport — tests viewport overflow
    let workers: Vec<_> = (0..50)
        .map(|i| make_worker(&format!("w-{i:02}"), "doing", &format!("task {i}")))
        .collect();
    let frame = render_dashboard(&workers, &zero_stats(), &empty_backlog(), 120, 10);
    // Should contain some workers but not all 50
    assert!(frame.contains("w-00"));
    // Worker 49 should NOT be visible in initial viewport
}

#[test]
fn render_triage_empty_activity_no_panic() {
    let frame = render_triage(&[], &[], 120, 30);
    assert!(frame.contains("GARDENER"));
}

#[test]
fn render_report_view_empty_report() {
    let frame = render_report_view("", "", 120, 30);
    // Should not panic with empty path and empty content
}

#[test]
fn render_report_view_very_long_content_truncated() {
    // Report with 1000 lines — should be truncated to fit height
    let report = (0..1000).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let frame = render_report_view("/tmp/report.md", &report, 120, 30);
    // Should contain early lines
    assert!(frame.contains("line 0"));
    // Should NOT contain all 1000 lines (height is 30)
}

#[test]
fn fake_terminal_draw_shutdown_does_not_produce_drawn_frame() {
    // GAP G2: FakeTerminal's draw_shutdown_screen doesn't call draw()
    let terminal = FakeTerminal::new(true);
    terminal.draw_shutdown_screen("title", "msg").unwrap();
    assert!(terminal.drawn_frames().is_empty(), "GAP G2: shutdown screen not in drawn_frames");
    assert_eq!(terminal.shutdown_screens().len(), 1, "but IS in shutdown_screens");
}

#[test]
fn fake_filesystem_exists_does_not_see_directories() {
    // GAP G3: create_dir_all creates dirs but exists() only checks files
    let fs = FakeFileSystem::default();
    fs.create_dir_all(Path::new("/some/dir")).unwrap();
    // This documents the gap: exists returns false for directories
    assert!(!fs.exists(Path::new("/some/dir")));
}
```

### Success criteria
- All tests compile and pass
- Boundary condition tests prove no panics occur
- Bug tests document B4 behavior with comments
- Gap tests document G2/G3 infrastructure limitations

---

## Testing Strategy

**All phases**: In-process tests using fakes. No PTY, no real filesystem, no real terminal.

**Test isolation**: No `#[serial]` needed since these tests don't touch thread-locals or
environment variables (except the env var parsing tests, which should use
`action_for_key_with_mode` directly with a bool, not the env-reading function).

**CI**: All phases run in `cargo nextest run`. No new dependencies needed.

**Bug documentation pattern**: Tests that expose bugs should use this format:
```rust
#[test]
fn descriptive_name() {
    // BUG Bn: Brief description of the bug
    // Current behavior: what happens now
    // Expected behavior: what should happen
    // Location: file.rs:line
    <test body asserting current (buggy) behavior>
}
```

This ensures bugs are discovered, documented, and tracked even before fixes are applied.

---

## References

- `tools/gardener/src/agent/claude.rs` — NDJSON parsing, `result` event handling
- `tools/gardener/src/agent/codex.rs` — `turn.failed`/`error` forward scan, `turn.completed` reverse scan
- `tools/gardener/src/protocol.rs` — `map_claude_event` / `map_codex_event` event classification
- `tools/gardener/src/triage.rs` — `run_triage`, `ensure_profile_for_run`, `triage_needed`
- `tools/gardener/src/triage_discovery.rs` — `run_discovery`, `DiscoveryAssessment`
- `tools/gardener/src/output_envelope.rs` — `parse_last_envelope`
- `tools/gardener/src/repo_intelligence.rs` — `read_profile`, `write_profile`, `current_head_sha`
- `tools/gardener/src/triage_agent_detection.rs` — `detect_agent`, `is_non_interactive`
- `tools/gardener/src/hotkeys.rs` — `action_for_key_with_mode`, `operator_hotkeys_enabled`
- `tools/gardener/src/tui.rs` — `render_dashboard`, `render_triage`, `render_report_view`
- `tools/gardener/src/runtime/mod.rs` — `FakeTerminal`, `FakeProcessRunner`, `FakeFileSystem`
- `tools/gardener/src/worker_pool.rs` — `run_worker_pool_fsm`, `handle_hotkeys`
- `tools/gardener/src/worker.rs` — `execute_task_live` (untested)
