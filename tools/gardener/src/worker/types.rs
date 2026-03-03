use crate::types::WorkerState;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkerLogEvent {
    pub state: WorkerState,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeardownReport {
    pub merge_verified: bool,
    pub session_torn_down: bool,
    pub sandbox_torn_down: bool,
    pub worktree_cleaned: bool,
    pub state_cleared: bool,
    pub main_updated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRunSummary {
    pub worker_id: String,
    pub session_id: String,
    pub final_state: WorkerState,
    pub logs: Vec<WorkerLogEvent>,
    pub teardown: Option<TeardownReport>,
    pub failure_reason: Option<String>,
}

/// All the state needed by a merge worker to run merge-and-teardown
/// independently of the doing worker that produced it.
pub struct MergeRequest {
    pub slot_idx: usize,
    pub task_id: String,
    pub task_summary: String,
    pub attempt_count: i64,
    pub worker_id: String,
    pub session_id: String,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub pr_number: u64,
    pub logs: Vec<WorkerLogEvent>,
    pub handoff_evidence_bundle: Option<PathBuf>,
}

/// Discriminates between a task that completed in-worker and one that needs
/// to be handed off to the merge worker.
pub enum WorkerOutcome {
    Completed(WorkerRunSummary),
    HandoffToMerge(MergeRequest),
}

#[derive(Debug, Clone)]
pub enum WorkerStreamEvent {
    ToolCommand {
        task_id: String,
        command: String,
    },
    StateChanged {
        _task_id: String,
        state: String,
        details: String,
    },
}

pub const PROMPT_LINE_COMMAND_LIMIT: usize = 220;
