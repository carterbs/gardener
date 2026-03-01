use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonInteractiveReason {
    ClaudeCodeEnv,
    CodexThreadEnv,
    CiEnv,
    NonTtyStdin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerState {
    Understand,
    Planning,
    Doing,
    Gitting,
    Reviewing,
    Merging,
    Seeding,
    Complete,
    Failed,
    Parked,
}

impl WorkerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Understand => "understand",
            Self::Planning => "planning",
            Self::Doing => "doing",
            Self::Gitting => "gitting",
            Self::Reviewing => "reviewing",
            Self::Merging => "merging",
            Self::Seeding => "seeding",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Parked => "parked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerActivityState {
    Claimed,
    Starting,
    WorktreePreparing,
    WorktreeReady,
    Understand,
    Planning,
    Doing,
    Commit,
    Gitting,
    GittingRemediation,
    PrCreating,
    Reviewing,
    Merging,
    MergePolling,
    MergeFromMain,
    MergeRemediation,
    CiFailureRemediation,
    PostMergeValidation,
    Teardown,
    Complete,
    Failed,
    Parked,
}

impl WorkerActivityState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Starting => "starting",
            Self::WorktreePreparing => "worktree_preparing",
            Self::WorktreeReady => "worktree_ready",
            Self::Understand => "understand",
            Self::Planning => "planning",
            Self::Doing => "doing",
            Self::Commit => "commit",
            Self::Gitting => "gitting",
            Self::GittingRemediation => "gitting_remediation",
            Self::PrCreating => "pr_creating",
            Self::Reviewing => "reviewing",
            Self::Merging => "merging",
            Self::MergePolling => "merge_polling",
            Self::MergeFromMain => "merge_from_main",
            Self::MergeRemediation => "merge_remediation",
            Self::CiFailureRemediation => "ci_failure_remediation",
            Self::PostMergeValidation => "post_merge_validation",
            Self::Teardown => "teardown",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Parked => "parked",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentKind, WorkerActivityState, WorkerState};

    #[test]
    fn agent_kind_as_str_covers_all_variants() {
        assert_eq!(AgentKind::Claude.as_str(), "claude");
        assert_eq!(AgentKind::Codex.as_str(), "codex");
    }

    #[test]
    fn worker_state_as_str_covers_all_variants() {
        assert_eq!(WorkerState::Understand.as_str(), "understand");
        assert_eq!(WorkerState::Planning.as_str(), "planning");
        assert_eq!(WorkerState::Doing.as_str(), "doing");
        assert_eq!(WorkerState::Gitting.as_str(), "gitting");
        assert_eq!(WorkerState::Reviewing.as_str(), "reviewing");
        assert_eq!(WorkerState::Merging.as_str(), "merging");
        assert_eq!(WorkerState::Seeding.as_str(), "seeding");
        assert_eq!(WorkerState::Complete.as_str(), "complete");
        assert_eq!(WorkerState::Failed.as_str(), "failed");
        assert_eq!(WorkerState::Parked.as_str(), "parked");
    }

    #[test]
    fn worker_activity_state_as_str_covers_all_variants() {
        assert_eq!(WorkerActivityState::Claimed.as_str(), "claimed");
        assert_eq!(WorkerActivityState::Starting.as_str(), "starting");
        assert_eq!(WorkerActivityState::WorktreePreparing.as_str(), "worktree_preparing");
        assert_eq!(WorkerActivityState::WorktreeReady.as_str(), "worktree_ready");
        assert_eq!(WorkerActivityState::Understand.as_str(), "understand");
        assert_eq!(WorkerActivityState::Planning.as_str(), "planning");
        assert_eq!(WorkerActivityState::Doing.as_str(), "doing");
        assert_eq!(WorkerActivityState::Commit.as_str(), "commit");
        assert_eq!(WorkerActivityState::Gitting.as_str(), "gitting");
        assert_eq!(WorkerActivityState::GittingRemediation.as_str(), "gitting_remediation");
        assert_eq!(WorkerActivityState::PrCreating.as_str(), "pr_creating");
        assert_eq!(WorkerActivityState::Reviewing.as_str(), "reviewing");
        assert_eq!(WorkerActivityState::Merging.as_str(), "merging");
        assert_eq!(WorkerActivityState::MergePolling.as_str(), "merge_polling");
        assert_eq!(WorkerActivityState::MergeFromMain.as_str(), "merge_from_main");
        assert_eq!(WorkerActivityState::MergeRemediation.as_str(), "merge_remediation");
        assert_eq!(WorkerActivityState::CiFailureRemediation.as_str(), "ci_failure_remediation");
        assert_eq!(WorkerActivityState::PostMergeValidation.as_str(), "post_merge_validation");
        assert_eq!(WorkerActivityState::Teardown.as_str(), "teardown");
        assert_eq!(WorkerActivityState::Complete.as_str(), "complete");
        assert_eq!(WorkerActivityState::Failed.as_str(), "failed");
        assert_eq!(WorkerActivityState::Parked.as_str(), "parked");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCommandResolution {
    pub command: String,
    pub startup_validate_on_boot: bool,
    pub startup_validation_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeScope {
    pub process_cwd: PathBuf,
    pub repo_root: Option<PathBuf>,
    pub working_dir: PathBuf,
}
