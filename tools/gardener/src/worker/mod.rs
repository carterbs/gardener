mod evidence;
mod merge_phase;
mod simulated;
mod stream_events;
mod types;
mod worker_doing;
mod worktree_naming;

pub(crate) use merge_phase::execute_merge_phase;
pub use merge_phase::{MAX_MERGE_REMEDIATION, MERGEABILITY_POLL_INTERVAL, MERGEABILITY_POLL_MAX};
pub(crate) use stream_events::{clear_state_sink, install_state_sink};
pub use types::{MergeRequest, WorkerOutcome, WorkerRunSummary, WorkerStreamEvent};
pub(crate) use worker_doing::execute_task;
pub(crate) use worktree_naming::{worktree_branch_for, worktree_path_for};
