//! Serializable types for session recording.
//!
//! A recording is a JSONL file where each line is a `RecordEntry` JSON object.

use crate::backlog_store::BacklogTask;
use crate::priority::Priority;
use crate::task_identity::TaskKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── BacklogTaskRecord ─────────────────────────────────────────────────────────

/// Snapshot of a single backlog task, captured at session start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacklogTaskRecord {
    pub task_id: String,
    pub kind: TaskKind,
    pub title: String,
    pub details: String,
    pub rationale: String,
    pub scope_key: String,
    pub priority: Priority,
    /// String form of `TaskStatus` (ready / leased / in_progress / complete / …)
    pub status: String,
    pub last_updated: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub source: String,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
    pub attempt_count: i64,
    pub created_at: i64,
}

impl From<BacklogTask> for BacklogTaskRecord {
    fn from(t: BacklogTask) -> Self {
        Self {
            task_id: t.task_id,
            kind: t.kind,
            title: t.title,
            details: t.details,
            rationale: t.rationale,
            scope_key: t.scope_key,
            priority: t.priority,
            status: t.status.as_str().to_string(),
            last_updated: t.last_updated,
            lease_owner: t.lease_owner,
            lease_expires_at: t.lease_expires_at,
            source: t.source,
            related_pr: t.related_pr,
            related_branch: t.related_branch,
            attempt_count: t.attempt_count,
            created_at: t.created_at,
        }
    }
}

// ── ProcessRequestRecord ──────────────────────────────────────────────────────

/// Serializable mirror of `runtime::ProcessRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRequestRecord {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
}

// ── ProcessOutputRecord ───────────────────────────────────────────────────────

const LARGE_OUTPUT_THRESHOLD: usize = 64 * 1024; // 64 KB

/// Serializable mirror of `runtime::ProcessOutput`, with optional truncation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOutputRecord {
    pub exit_code: i32,
    /// Full stdout, or `<hash:sha256:XXXXXXXXXXXXXXXX>` when truncated.
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub stdout_truncated: bool,
}

impl ProcessOutputRecord {
    pub fn from_output(exit_code: i32, stdout: String, stderr: String) -> Self {
        if stdout.len() > LARGE_OUTPUT_THRESHOLD {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(stdout.as_bytes());
            // Take the first 8 bytes (16 hex chars)
            let prefix = hex_bytes(&hash[..8]);
            Self {
                exit_code,
                stdout: format!("<hash:sha256:{prefix}>"),
                stderr,
                stdout_truncated: true,
            }
        } else {
            Self {
                exit_code,
                stdout,
                stderr,
                stdout_truncated: false,
            }
        }
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── RecordEntry ───────────────────────────────────────────────────────────────

/// The top-level tagged enum that is serialized as a single JSONL line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecordEntry {
    SessionStart(SessionStartRecord),
    BacklogSnapshot(BacklogSnapshotRecord),
    ProcessCall(ProcessCallRecord),
    AgentTurn(AgentTurnRecord),
    BacklogMutation(BacklogMutationRecord),
    SessionEnd(SessionEndRecord),
}

// ── Concrete record types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartRecord {
    pub run_id: String,
    pub recorded_at_unix_ns: u64,
    pub gardener_version: String,
    /// Full `AppConfig` serialized as JSON.
    pub config_snapshot: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacklogSnapshotRecord {
    pub tasks: Vec<BacklogTaskRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCallRecord {
    pub seq: u64,
    pub timestamp_ns: u64,
    pub worker_id: String,
    pub thread_id: String,
    pub request: ProcessRequestRecord,
    pub result: ProcessOutputRecord,
    pub duration_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnRecord {
    pub seq: u64,
    pub timestamp_ns: u64,
    pub worker_id: String,
    /// String form of `WorkerState`
    pub state: String,
    /// `"success"` or `"failure"`
    pub terminal: String,
    /// The `step.payload` value consumed by the FSM.
    pub payload: Value,
    pub diagnostic_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacklogMutationRecord {
    pub seq: u64,
    pub timestamp_ns: u64,
    pub worker_id: String,
    /// e.g. `"claim_next"`, `"mark_in_progress"`, `"mark_complete"`, `"release_lease"`
    pub operation: String,
    pub task_id: String,
    pub result_ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEndRecord {
    pub completed_tasks: u64,
    pub total_duration_ns: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_output_record_truncates_large_stdout() {
        let big = "x".repeat(65_537);
        let rec = ProcessOutputRecord::from_output(0, big, String::new());
        assert!(rec.stdout_truncated);
        assert!(rec.stdout.starts_with("<hash:sha256:"));
        assert_eq!(rec.exit_code, 0);
    }

    #[test]
    fn process_output_record_keeps_small_stdout() {
        let small = "hello world".to_string();
        let rec = ProcessOutputRecord::from_output(0, small.clone(), String::new());
        assert!(!rec.stdout_truncated);
        assert_eq!(rec.stdout, small);
    }

    #[test]
    fn record_entry_round_trips_json() {
        let entry = RecordEntry::SessionStart(SessionStartRecord {
            run_id: "run-1".to_string(),
            recorded_at_unix_ns: 12345,
            gardener_version: "0.1.0".to_string(),
            config_snapshot: serde_json::json!({"test": true}),
        });
        let json = serde_json::to_string(&entry).expect("serialize");
        let back: RecordEntry = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back, RecordEntry::SessionStart(_)));
    }
}
