//! Replay infrastructure: load a recording and re-drive the FSM deterministically.
//!
//! Populated in Phase 4 of the session-replay implementation plan.

use crate::errors::GardenerError;
use crate::replay::recording::{
    AgentTurnRecord, BacklogMutationRecord, BacklogTaskRecord, ProcessCallRecord,
    RecordEntry, SessionStartRecord,
};
use crate::runtime::{ProcessOutput, ProcessRequest, ProcessRunner};
use crate::agent::AgentAdapter;
use crate::protocol::{StepResult, AgentTerminal};
use crate::types::AgentKind;
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
