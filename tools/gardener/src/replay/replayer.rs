//! Replay infrastructure: load a recording and re-drive the FSM deterministically.

use crate::agent::AgentAdapter;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::protocol::{AgentTerminal, StepResult};
use crate::replay::recording::{
    AgentTurnRecord, BacklogMutationRecord, BacklogTaskRecord, ProcessCallRecord,
    RecordEntry, SessionStartRecord,
};
use crate::runtime::{ProcessOutput, ProcessRequest, ProcessRunner};
use crate::types::{AgentKind, RuntimeScope};
use crate::worker::execute_task;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

// ── SessionRecording ──────────────────────────────────────────────────────────

/// A parsed recording file, ready for replay.
pub struct SessionRecording {
    pub header: SessionStartRecord,
    pub backlog: Vec<BacklogTaskRecord>,
    pub entries: Vec<RecordEntry>,
}

impl SessionRecording {
    /// Load and parse a JSONL recording file.
    pub fn load(path: &Path) -> Result<Self, GardenerError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| GardenerError::Io(e.to_string()))?;
        let mut header: Option<SessionStartRecord> = None;
        let mut backlog: Vec<BacklogTaskRecord> = Vec::new();
        let mut entries: Vec<RecordEntry> = Vec::new();
        for (idx, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let entry: RecordEntry = serde_json::from_str(line).map_err(|e| {
                GardenerError::Process(format!("recording line {}: {e}", idx + 1))
            })?;
            match &entry {
                RecordEntry::SessionStart(s) => {
                    header = Some(s.clone());
                }
                RecordEntry::BacklogSnapshot(snap) => {
                    backlog = snap.tasks.clone();
                }
                _ => {}
            }
            entries.push(entry);
        }
        let header = header.ok_or_else(|| {
            GardenerError::Process("recording has no SessionStart entry".to_string())
        })?;
        Ok(Self {
            header,
            backlog,
            entries,
        })
    }

    /// Return the set of worker IDs that appear in the recording.
    pub fn worker_ids(&self) -> Vec<String> {
        let mut ids = std::collections::BTreeSet::new();
        for entry in &self.entries {
            match entry {
                RecordEntry::ProcessCall(r) => {
                    ids.insert(r.worker_id.clone());
                }
                RecordEntry::AgentTurn(r) => {
                    ids.insert(r.worker_id.clone());
                }
                RecordEntry::BacklogMutation(r) => {
                    ids.insert(r.worker_id.clone());
                }
                _ => {}
            }
        }
        ids.into_iter().collect()
    }

    /// All process calls for a specific worker, in recording order.
    pub fn process_calls_for(&self, worker_id: &str) -> Vec<ProcessCallRecord> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                RecordEntry::ProcessCall(r) if r.worker_id == worker_id => Some(r.clone()),
                _ => None,
            })
            .collect()
    }

    /// All agent turns for a specific worker, in recording order.
    pub fn agent_turns_for(&self, worker_id: &str) -> Vec<AgentTurnRecord> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                RecordEntry::AgentTurn(r) if r.worker_id == worker_id => Some(r.clone()),
                _ => None,
            })
            .collect()
    }

    /// All backlog mutations across all workers, in recording order.
    pub fn backlog_mutations(&self) -> Vec<BacklogMutationRecord> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                RecordEntry::BacklogMutation(r) => Some(r.clone()),
                _ => None,
            })
            .collect()
    }
}

// ── ReplayProcessRunner ───────────────────────────────────────────────────────

/// Mismatch between expected and actual process request during replay.
#[derive(Debug, Clone)]
pub struct RequestMismatch {
    pub position: usize,
    pub expected_program: String,
    pub actual_program: String,
}

/// Per-worker `ProcessRunner` that returns pre-recorded responses in FIFO order.
pub struct ReplayProcessRunner {
    responses: Mutex<VecDeque<ProcessOutput>>,
    expected_requests: Mutex<VecDeque<ProcessCallRecord>>,
    actual_requests: Mutex<Vec<ProcessRequest>>,
    next_handle: Mutex<u64>,
}

impl ReplayProcessRunner {
    pub fn from_recording(recording: &SessionRecording, worker_id: &str) -> Self {
        let calls = recording.process_calls_for(worker_id);
        let responses: VecDeque<ProcessOutput> = calls
            .iter()
            .map(|c| {
                ProcessOutput {
                    exit_code: c.result.exit_code,
                    stdout: if c.result.stdout_truncated {
                        // Truncated in recording; replay with empty stdout
                        String::new()
                    } else {
                        c.result.stdout.clone()
                    },
                    stderr: c.result.stderr.clone(),
                }
            })
            .collect();
        let expected_requests = calls.into_iter().collect();
        Self {
            responses: Mutex::new(responses),
            expected_requests: Mutex::new(expected_requests),
            actual_requests: Mutex::new(Vec::new()),
            next_handle: Mutex::new(0),
        }
    }

    /// After replay, verify that the actual process requests match the recorded ones.
    pub fn verify_request_alignment(&self) -> Vec<RequestMismatch> {
        let actual = self.actual_requests.lock().expect("actual requests lock");
        let expected = self.expected_requests.lock().expect("expected requests lock");
        actual
            .iter()
            .zip(expected.iter())
            .enumerate()
            .filter_map(|(i, (act, exp))| {
                if act.program != exp.request.program {
                    Some(RequestMismatch {
                        position: i,
                        expected_program: exp.request.program.clone(),
                        actual_program: act.program.clone(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

impl ProcessRunner for ReplayProcessRunner {
    fn spawn(&self, request: ProcessRequest) -> Result<u64, GardenerError> {
        self.actual_requests
            .lock()
            .expect("actual requests lock")
            .push(request);
        let mut h = self.next_handle.lock().expect("next handle lock");
        let handle = *h;
        *h += 1;
        Ok(handle)
    }

    fn wait(&self, _handle: u64) -> Result<ProcessOutput, GardenerError> {
        self.responses
            .lock()
            .expect("responses lock")
            .pop_front()
            .ok_or_else(|| {
                GardenerError::Process("replay: no more recorded responses".to_string())
            })
    }

    fn kill(&self, _handle: u64) -> Result<(), GardenerError> {
        Ok(())
    }

    /// Deliver the recorded stdout through the same chunk-based line-splitting
    /// logic as `ProductionProcessRunner`, so that replays faithfully reproduce
    /// the production streaming behaviour — including any existing bugs in
    /// `append_and_flush_lines` (e.g. `[..end]` vs `[cursor..end]`).
    fn wait_with_line_stream(
        &self,
        handle: u64,
        on_stdout_line: &mut dyn FnMut(&str),
        on_stderr_line: &mut dyn FnMut(&str),
    ) -> Result<ProcessOutput, GardenerError> {
        let output = self.wait(handle)?;
        // Treat the entire recorded stdout as a single OS-read chunk,
        // matching the worst-case production scenario where all output
        // arrives in one read() call before the process exits.
        let bytes = output.stdout.as_bytes();
        let mut line_buffer: Vec<u8> = Vec::new();
        line_buffer.extend_from_slice(bytes);
        let mut cursor = 0usize;
        while let Some(pos) = line_buffer[cursor..].iter().position(|b| *b == b'\n') {
            let end = cursor + pos;
            // Mirrors append_and_flush_lines in runtime/mod.rs verbatim.
            let line = String::from_utf8_lossy(&line_buffer[..end]);
            on_stdout_line(line.trim_end_matches('\r'));
            cursor = end + 1;
        }
        if cursor > 0 {
            line_buffer.drain(..cursor);
        }
        if !line_buffer.is_empty() {
            let line = String::from_utf8_lossy(&line_buffer);
            on_stdout_line(line.trim_end_matches('\r'));
        }
        for line in output.stderr.lines() {
            on_stderr_line(line);
        }
        Ok(output)
    }
}

// ── ReplayAgentAdapter ────────────────────────────────────────────────────────

use crate::agent::AdapterContext;

/// Per-worker `AgentAdapter` that returns pre-recorded `StepResult`s in FIFO order.
pub struct ReplayAgentAdapter {
    responses: Mutex<VecDeque<StepResult>>,
    backend: AgentKind,
}

impl ReplayAgentAdapter {
    pub fn from_recording(
        recording: &SessionRecording,
        worker_id: &str,
        backend: AgentKind,
    ) -> Self {
        let turns = recording.agent_turns_for(worker_id);
        let responses: VecDeque<StepResult> = turns
            .into_iter()
            .map(|t| StepResult {
                terminal: if t.terminal == "success" {
                    AgentTerminal::Success
                } else {
                    AgentTerminal::Failure
                },
                events: Vec::new(),
                payload: t.payload,
                diagnostics: Vec::new(),
            })
            .collect();
        Self {
            responses: Mutex::new(responses),
            backend,
        }
    }
}

impl AgentAdapter for ReplayAgentAdapter {
    fn backend(&self) -> AgentKind {
        self.backend
    }

    fn probe_capabilities(
        &self,
        _process_runner: &dyn ProcessRunner,
    ) -> Result<crate::agent::AdapterCapabilities, GardenerError> {
        Ok(crate::agent::AdapterCapabilities {
            backend: Some(self.backend),
            version: Some("replay".to_string()),
            supports_json: false,
            supports_stream_json: false,
            supports_max_turns: false,
            supports_output_schema: false,
            supports_output_last_message: false,
            supports_listen_stdio: false,
            supports_stdin_prompt: false,
        })
    }

    fn execute(
        &self,
        _process_runner: &dyn ProcessRunner,
        _context: &AdapterContext,
        _prompt: &str,
        _on_event: Option<&mut dyn FnMut(&crate::protocol::AgentEvent)>,
    ) -> Result<StepResult, GardenerError> {
        self.responses
            .lock()
            .expect("responses lock")
            .pop_front()
            .ok_or_else(|| {
                GardenerError::Process("replay: no more recorded agent turns".to_string())
            })
    }
}

// ── Replay outcome types ──────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ReplayOutcome {
    pub worker_id: String,
    pub final_state: crate::types::WorkerState,
    pub request_mismatches: Vec<RequestMismatch>,
    pub passed: bool,
}

#[derive(Debug)]
pub struct SessionReplayReport {
    pub outcomes: Vec<ReplayOutcome>,
    pub all_passed: bool,
}

// ── replay_worker_task ────────────────────────────────────────────────────────

/// Replay a single worker's task execution from a recording.
///
/// Uses `ReplayProcessRunner` pre-seeded with the worker's recorded subprocess
/// responses.  The real agent adapter (Claude/Codex) re-parses those responses
/// so the FSM drives through the same state machine as the original run.
pub fn replay_worker_task(
    recording: &SessionRecording,
    worker_id: &str,
    cfg: &AppConfig,
    scope: &RuntimeScope,
) -> Result<ReplayOutcome, GardenerError> {
    // Find the task that was claimed by this worker
    let claim = recording
        .backlog_mutations()
        .into_iter()
        .find(|m| m.worker_id == worker_id && m.operation == "claim_next")
        .ok_or_else(|| {
            GardenerError::Process(format!(
                "replay: no claim_next mutation found for worker {worker_id}"
            ))
        })?;
    let task = recording
        .backlog
        .iter()
        .find(|t| t.task_id == claim.task_id)
        .ok_or_else(|| {
            GardenerError::Process(format!(
                "replay: task {} not found in backlog snapshot",
                claim.task_id
            ))
        })?;

    let runner = ReplayProcessRunner::from_recording(recording, worker_id);
    let summary = execute_task(
        cfg,
        &runner,
        scope,
        worker_id,
        &task.task_id,
        &task.title,
        task.attempt_count,
    )?;

    let mismatches = runner.verify_request_alignment();
    let passed = mismatches.is_empty();
    Ok(ReplayOutcome {
        worker_id: worker_id.to_string(),
        final_state: summary.final_state,
        request_mismatches: mismatches,
        passed,
    })
}

// ── replay_session ────────────────────────────────────────────────────────────

/// Replay a full recorded session, one worker at a time (serial, not parallel).
///
/// Returns a `SessionReplayReport` with per-worker pass/fail and an overall result.
/// Serial replay catches FSM logic regressions regardless of concurrency ordering.
pub fn replay_session(
    recording: &SessionRecording,
    cfg: &AppConfig,
    scope: &RuntimeScope,
) -> Result<SessionReplayReport, GardenerError> {
    let worker_ids = recording.worker_ids();
    let mut outcomes = Vec::with_capacity(worker_ids.len());
    for worker_id in &worker_ids {
        let outcome = replay_worker_task(recording, worker_id, cfg, scope)?;
        outcomes.push(outcome);
    }
    let all_passed = outcomes.iter().all(|o| o.passed);
    Ok(SessionReplayReport {
        outcomes,
        all_passed,
    })
}

#[cfg(test)]
mod tests {
    use super::{ReplayProcessRunner, SessionRecording};
    use crate::runtime::ProcessRunner;
    use crate::replay::recording::{
        AgentTurnRecord, BacklogMutationRecord, BacklogSnapshotRecord, ProcessCallRecord,
        ProcessRequestRecord, RecordEntry, SessionStartRecord,
    };
    use std::path::Path;

    fn write_recording(path: &Path, entries: Vec<RecordEntry>) {
        let payload = entries
            .into_iter()
            .map(|entry| serde_json::to_string(&entry).expect("serialize"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(path, payload).expect("write recording");
    }

    #[test]
    fn load_requires_session_start_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("missing_session.jsonl");
        write_recording(
            &path,
            vec![RecordEntry::BacklogSnapshot(BacklogSnapshotRecord {
                tasks: Vec::new(),
            })],
        );
        let err = match SessionRecording::load(&path) {
            Ok(_) => panic!("expected missing session start to fail"),
            Err(err) => err,
        };
        assert_eq!(err.to_string(), "process error: recording has no SessionStart entry");
    }

    #[test]
    fn session_recording_filters_ids_and_records_by_worker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("replay.jsonl");
        let entries = vec![
            RecordEntry::SessionStart(SessionStartRecord {
                run_id: "run-1".to_string(),
                recorded_at_unix_ns: 1,
                gardener_version: "0.0.0".to_string(),
                config_snapshot: serde_json::json!({}),
            }),
            RecordEntry::BacklogSnapshot(BacklogSnapshotRecord {
                tasks: Vec::new(),
            }),
            RecordEntry::ProcessCall(ProcessCallRecord {
                seq: 1,
                timestamp_ns: 1,
                worker_id: "worker-a".to_string(),
                thread_id: "main".to_string(),
                request: ProcessRequestRecord {
                    program: "echo".to_string(),
                    args: vec!["alpha".to_string()],
                    cwd: Some("/tmp".to_string()),
                },
                result: crate::replay::recording::ProcessOutputRecord::from_output(
                    0,
                    "stdout line\n".to_string(),
                    String::new(),
                ),
                duration_ns: 10,
            }),
            RecordEntry::ProcessCall(ProcessCallRecord {
                seq: 2,
                timestamp_ns: 2,
                worker_id: "worker-b".to_string(),
                thread_id: "main".to_string(),
                request: ProcessRequestRecord {
                    program: "ls".to_string(),
                    args: vec![".".to_string()],
                    cwd: Some("/tmp".to_string()),
                },
                result: crate::replay::recording::ProcessOutputRecord::from_output(
                    0,
                    "beta".to_string(),
                    String::new(),
                ),
                duration_ns: 20,
            }),
            RecordEntry::AgentTurn(AgentTurnRecord {
                seq: 3,
                timestamp_ns: 3,
                worker_id: "worker-a".to_string(),
                state: "doing".to_string(),
                terminal: "success".to_string(),
                payload: serde_json::json!({ "terminal": "success" }),
                diagnostic_count: 0,
            }),
            RecordEntry::BacklogMutation(BacklogMutationRecord {
                seq: 4,
                timestamp_ns: 4,
                worker_id: "worker-a".to_string(),
                operation: "claim_next".to_string(),
                task_id: "task-1".to_string(),
                result_ok: true,
            }),
            RecordEntry::BacklogMutation(BacklogMutationRecord {
                seq: 5,
                timestamp_ns: 5,
                worker_id: "worker-b".to_string(),
                operation: "release_lease".to_string(),
                task_id: "task-2".to_string(),
                result_ok: true,
            }),
        ];
        write_recording(&path, entries);

        let recording = SessionRecording::load(&path).expect("load recording");
        assert_eq!(recording.worker_ids(), vec!["worker-a".to_string(), "worker-b".to_string()]);
        assert_eq!(recording.process_calls_for("worker-a").len(), 1);
        assert_eq!(recording.backlog_mutations().len(), 2);
        assert_eq!(recording.agent_turns_for("worker-a").len(), 1);
    }

    #[test]
    fn replay_process_runner_matches_expected_request_sequence() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("replay-matcher.jsonl");
        let entries = vec![
            RecordEntry::SessionStart(SessionStartRecord {
                run_id: "run-1".to_string(),
                recorded_at_unix_ns: 1,
                gardener_version: "0.0.0".to_string(),
                config_snapshot: serde_json::json!({}),
            }),
            RecordEntry::BacklogSnapshot(BacklogSnapshotRecord {
                tasks: Vec::new(),
            }),
            RecordEntry::ProcessCall(ProcessCallRecord {
                seq: 1,
                timestamp_ns: 1,
                worker_id: "worker-a".to_string(),
                thread_id: "main".to_string(),
                request: ProcessRequestRecord {
                    program: "echo".to_string(),
                    args: vec!["first".to_string()],
                    cwd: Some("/tmp".to_string()),
                },
                result: crate::replay::recording::ProcessOutputRecord::from_output(
                    0,
                    "hello\nworld\n".to_string(),
                    String::new(),
                ),
                duration_ns: 10,
            }),
            RecordEntry::ProcessCall(ProcessCallRecord {
                seq: 2,
                timestamp_ns: 2,
                worker_id: "worker-a".to_string(),
                thread_id: "main".to_string(),
                request: ProcessRequestRecord {
                    program: "printf".to_string(),
                    args: vec!["second".to_string()],
                    cwd: Some("/tmp".to_string()),
                },
                result: crate::replay::recording::ProcessOutputRecord::from_output(
                    0,
                    String::new(),
                    "stderr".to_string(),
                ),
                duration_ns: 20,
            }),
            RecordEntry::ProcessCall(ProcessCallRecord {
                seq: 3,
                timestamp_ns: 3,
                worker_id: "worker-b".to_string(),
                thread_id: "main".to_string(),
                request: ProcessRequestRecord {
                    program: "git".to_string(),
                    args: vec!["status".to_string()],
                    cwd: Some("/tmp".to_string()),
                },
                result: crate::replay::recording::ProcessOutputRecord::from_output(
                    0,
                    "ok".to_string(),
                    String::new(),
                ),
                duration_ns: 30,
            }),
        ];
        write_recording(&path, entries);
        let recording = SessionRecording::load(&path).expect("load recording");

        let runner = ReplayProcessRunner::from_recording(&recording, "worker-a");
        let mut lines = Vec::new();
        let handle = runner
                .spawn(crate::runtime::ProcessRequest {
                program: "echo".to_string(),
                args: vec!["first".to_string()],
                cwd: Some("/tmp".into()),
            })
            .expect("spawn");
        let output = runner
            .wait_with_line_stream(
                handle,
                &mut |line: &str| lines.push(line.to_string()),
                &mut |_line| {},
            )
            .expect("stream");
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stderr, String::new());
        assert_eq!(lines, vec!["hello".to_string(), "hello\nworld".to_string()]);

        let mismatches = runner.verify_request_alignment();
        assert!(mismatches.is_empty());
    }

    #[test]
    fn replay_process_runner_detects_program_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("replay-mismatch.jsonl");
        let entries = vec![
            RecordEntry::SessionStart(SessionStartRecord {
                run_id: "run-1".to_string(),
                recorded_at_unix_ns: 1,
                gardener_version: "0.0.0".to_string(),
                config_snapshot: serde_json::json!({}),
            }),
            RecordEntry::BacklogSnapshot(BacklogSnapshotRecord {
                tasks: Vec::new(),
            }),
            RecordEntry::ProcessCall(ProcessCallRecord {
                seq: 1,
                timestamp_ns: 1,
                worker_id: "worker-a".to_string(),
                thread_id: "main".to_string(),
                request: ProcessRequestRecord {
                    program: "echo".to_string(),
                    args: vec!["first".to_string()],
                    cwd: Some("/tmp".to_string()),
                },
                result: crate::replay::recording::ProcessOutputRecord::from_output(
                    0,
                    String::new(),
                    String::new(),
                ),
                duration_ns: 10,
            }),
        ];
        write_recording(&path, entries);
        let recording = SessionRecording::load(&path).expect("load recording");

        let runner = ReplayProcessRunner::from_recording(&recording, "worker-a");
        assert!(
            runner
                .spawn(crate::runtime::ProcessRequest {
                    program: "printf".to_string(),
                    args: vec!["oops".to_string()],
                    cwd: None,
                })
                .is_ok()
        );
        runner
            .wait(0)
            .expect("consume expected output");
        let mismatches = runner.verify_request_alignment();
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].position, 0);
        assert_eq!(mismatches[0].expected_program, "echo");
    }
}
