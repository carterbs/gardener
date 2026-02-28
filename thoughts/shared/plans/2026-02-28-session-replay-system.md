# Session Replay System for Gardener

## Context

Gardener has had recurring reliability issues with git merge operations, BacklogStore corruption, and worktree lifecycle management. Currently there's no way to reproduce these failures systematically. The goal: let gardener run for hours in production, capture a recording, then replay that session deterministically in tests in 1-2 minutes. This gives us a growing corpus of real-world test data to prevent regressions.

## Approach: Three-Boundary Recording

Record at three boundaries where "our code" meets "the outside world":

1. **ProcessRunner boundary** - every subprocess call (git, gh, claude, sh) and its response
2. **Agent turn boundary** - the `StepResult` returned by `AgentAdapter::execute()` to the FSM
3. **BacklogStore state boundary** - initial task snapshot + mutation log

Replay substitutes `FakeProcessRunner` (populated from recording), `ReplayAgentAdapter` (returns recorded StepResults), and a pre-seeded BacklogStore. FakeClock makes it instant.

---

## Phase 1: Recording Types and Session Recorder

### New files
- `src/replay/mod.rs` - module root
- `src/replay/recording.rs` - `RecordEntry` tagged enum (serde-serializable)
- `src/replay/recorder.rs` - `SessionRecorder` global + thread-local worker ID

### `RecordEntry` variants
```
SessionStart { run_id, recorded_at_unix_ns, gardener_version, config_snapshot }
BacklogSnapshot { tasks: Vec<BacklogTaskRecord> }
ProcessCall { seq, timestamp_ns, worker_id, thread_id, request, result, duration_ns }
AgentTurn { seq, timestamp_ns, worker_id, state, terminal, payload, diagnostic_count }
BacklogMutation { seq, timestamp_ns, worker_id, operation, task_id, result_ok }
SessionEnd { completed_tasks, total_duration_ns }
```

### `SessionRecorder`
- Global singleton following the `RUN_LOGGER` pattern in `logging.rs:32-34` (OnceLock<Mutex<Option<...>>>)
- `init_session_recorder(path)` / `clear_session_recorder()` lifecycle
- `emit(entry: RecordEntry)` - serialize + append + newline (holds write lock)
- `AtomicU64` sequence counter for monotonic ordering across threads
- `SessionRecorder::disabled()` returns a no-op instance (zero overhead when not recording)

### Thread-local worker ID
```rust
thread_local! { static RECORDING_WORKER_ID: RefCell<String> = RefCell::new("pool".into()); }
pub fn set_recording_worker_id(id: &str) { ... }
pub fn get_recording_worker_id() -> String { ... }
```

### Files to modify
- `src/lib.rs` - add `pub mod replay;`
- `src/protocol.rs` - add `#[derive(Serialize, Deserialize)]` to `AgentTerminal` and `StepResult` (AgentEvent already has it)

---

## Phase 2: RecordingProcessRunner

### New in `src/replay/recorder.rs`

`RecordingProcessRunner` wraps any `Arc<dyn ProcessRunner>`:
- On `spawn()`: stores `ProcessRequest` keyed by handle in internal `HashMap<u64, ProcessRequest>`
- On `wait()` / `wait_with_line_stream()`: calls inner, then emits `ProcessCall` record pairing stored request with result
- On `kill()`: delegates, no recording needed
- Large stdout handling: if `stdout.len() > 64KB`, store SHA-256 hash prefix instead (`<hash:sha256:first16bytes>`), set `is_truncated = true`. The `AgentTurn` record has the actual payload the FSM needs.

Uses `get_recording_worker_id()` to tag each record.

### Files to modify
- `src/worker_pool.rs:159` - add `set_recording_worker_id(&worker_id)` inside the `scope_guard.spawn()` closure, right before `execute_task()`

---

## Phase 3: Agent Turn + BacklogStore Recording

### Agent turn recording
In `src/worker.rs`, function `run_agent_turn()` (line 802):
- After `adapter.execute()` returns `step` (line 873), emit an `AgentTurn` record with the full `step.payload` and `step.terminal`
- This is the critical replay data - the FSM consumes `step.payload` and `step.terminal` to make decisions

### BacklogStore recording
In `src/worker_pool.rs`:
- After `store.claim_next()` (line 108): emit `BacklogMutation { operation: "claim_next", task_id, ... }`
- After `store.mark_in_progress()` (line 126): emit mutation
- After `store.mark_complete()` (line 242): emit mutation
- After `store.release_lease()` (line 297): emit mutation

### BacklogSnapshot
In `src/lib.rs` around line 418-436 (where `backlog.startup.snapshot` is already logged):
- Emit `BacklogSnapshot` record with full task details (not truncated like the current otel log)

---

## Phase 4: Replay Infrastructure

### New file: `src/replay/replayer.rs`

#### `SessionRecording`
Parses a recording JSONL file into structured data:
```rust
pub struct SessionRecording {
    pub header: SessionStartRecord,
    pub backlog: Vec<BacklogTaskRecord>,
    pub entries: Vec<RecordEntry>,  // all entries in order
}
impl SessionRecording {
    pub fn load(path: &Path) -> Result<Self, GardenerError>;
    pub fn worker_ids(&self) -> Vec<String>;
    pub fn process_calls_for(&self, worker_id: &str) -> Vec<ProcessCallRecord>;
    pub fn agent_turns_for(&self, worker_id: &str) -> Vec<AgentTurnRecord>;
    pub fn backlog_mutations(&self) -> Vec<BacklogMutationRecord>;
}
```

#### `ReplayProcessRunner`
Per-worker, extends the `FakeProcessRunner` pattern:
```rust
pub struct ReplayProcessRunner {
    responses: Mutex<VecDeque<ProcessOutput>>,
    expected_requests: Mutex<VecDeque<ProcessRequestRecord>>,  // for assertion
    actual_requests: Mutex<Vec<ProcessRequest>>,  // for verification
}
impl ReplayProcessRunner {
    pub fn from_recording(recording: &SessionRecording, worker_id: &str) -> Self;
    pub fn verify_request_alignment(&self) -> Vec<RequestMismatch>;  // post-replay check
}
```
Implements `ProcessRunner` exactly like `FakeProcessRunner` (FIFO pop from queue).

#### `ReplayAgentAdapter`
Per-worker adapter returning recorded StepResults:
```rust
pub struct ReplayAgentAdapter {
    responses: Mutex<VecDeque<StepResult>>,
    backend: AgentKind,
}
impl ReplayAgentAdapter {
    pub fn from_recording(recording: &SessionRecording, worker_id: &str, backend: AgentKind) -> Self;
}
```
Implements `AgentAdapter` - `execute()` pops next StepResult from queue.

#### `replay_worker_task()`
Top-level replay function for a single worker:
```rust
pub fn replay_worker_task(
    recording: &SessionRecording,
    worker_id: &str,
    cfg: &AppConfig,
) -> Result<ReplayOutcome, GardenerError> {
    let process_runner = ReplayProcessRunner::from_recording(recording, worker_id);
    // Call execute_task() with the replay process runner
    // Compare WorkerRunSummary against recorded outcome
    // Return comparison result
}
```

#### `replay_session()`
Top-level replay function for full pool session:
```rust
pub fn replay_session(recording: &SessionRecording) -> Result<SessionReplayReport, GardenerError> {
    // For each worker_id in recording:
    //   - Build ReplayProcessRunner + ReplayAgentAdapter
    //   - Run execute_task() with replay components
    //   - Compare final_state against recorded outcome
    // Verify backlog mutation sequence matches
    // Return report with pass/fail per worker + overall
}
```

Serial replay (not parallel) - each worker replayed sequentially. This catches logic regressions. The BacklogStore mutation sequence captures concurrency ordering for verification.

---

## Phase 5: CLI + Production Integration

### Files to modify
- `src/lib.rs` - add `--record-session <path>` CLI flag to `Cli` struct
- `src/lib.rs:run_with_runtime()` - when flag set:
  - Call `init_session_recorder(path)` early
  - Wrap `runtime.process_runner` in `RecordingProcessRunner`
  - Emit `SessionStart` and `BacklogSnapshot` records
  - After pool completes, emit `SessionEnd` and `clear_session_recorder()`
- Support `GARDENER_RECORD_SESSION=<path>` env var as alternative

---

## Phase 6: Integration Tests

### New file: `tests/replay_integration.rs`

| Test | What it validates |
|------|-------------------|
| `round_trip_single_worker_happy_path` | Record a FakeProcessRunner session -> save -> load -> replay -> same WorkerRunSummary |
| `round_trip_gitting_recovery` | Record dirty-worktree recovery -> replay -> same failure/success |
| `round_trip_review_loop` | Record NeedsChanges -> Doing -> Reviewing -> Approve -> replay matches |
| `replay_catches_fsm_state_mismatch` | Intentionally mutate recording -> replay detects wrong final state |
| `replay_process_runner_returns_recorded_responses` | Unit: ReplayProcessRunner pops in order |
| `replay_agent_adapter_returns_recorded_step_results` | Unit: ReplayAgentAdapter pops correctly |
| `large_agent_output_hash_compressed` | Record claude-sized output -> verify hash truncation + AgentTurn still has payload |
| `recording_overhead_acceptable` | Benchmark: <5% overhead vs bare FakeProcessRunner |

---

## Key Files Reference

| File | Role | Change |
|------|------|--------|
| `src/replay/mod.rs` | **NEW** | Module root |
| `src/replay/recording.rs` | **NEW** | RecordEntry types |
| `src/replay/recorder.rs` | **NEW** | SessionRecorder, RecordingProcessRunner, thread-local worker ID |
| `src/replay/replayer.rs` | **NEW** | SessionRecording, ReplayProcessRunner, ReplayAgentAdapter, replay functions |
| `src/lib.rs` | MODIFY | Add `pub mod replay`, `--record-session` flag, recorder lifecycle |
| `src/protocol.rs` | MODIFY | Add Serialize/Deserialize to AgentTerminal, StepResult |
| `src/worker.rs:873` | MODIFY | Emit AgentTurn record after adapter.execute() |
| `src/worker_pool.rs:159` | MODIFY | Set thread-local worker ID before execute_task |
| `src/worker_pool.rs:108,126,242,297` | MODIFY | Emit BacklogMutation records |
| `tests/replay_integration.rs` | **NEW** | Round-trip and regression tests |

## Existing code to reuse

- `FakeProcessRunner` pattern (`runtime/mod.rs:950-1001`) - ReplayProcessRunner mirrors this exactly
- `JsonlLogger` / `append_run_log` global pattern (`logging.rs:32-34`) - SessionRecorder follows same OnceLock<Mutex> approach
- `BacklogStore::list_tasks()` (`backlog_store.rs:542`) - for BacklogSnapshot
- `AdapterFactory::register()` (`agent/factory.rs:30`) - to plug in ReplayAgentAdapter
- Phase10 test patterns (`tests/phase10_gitting_recovery_integration.rs`) - for integration test structure

## Verification

1. `cargo test --test replay_integration` - all round-trip tests pass
2. `cargo test` - all existing tests still pass (no regressions)
3. Manual: `gardener --record-session /tmp/session.jsonl --quit-after 1` produces valid JSONL
4. Manual: feed that JSONL into `replay_session()` via a test, verify pass
5. `cargo clippy` - no new warnings (project denies `clippy::unwrap_used`)
