use tokio::sync::oneshot;

use crate::errors::GardenerError;
use crate::priority::Priority;
use crate::task_identity::TaskKind;

pub(super) type StoreResult<T> = Result<T, GardenerError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Ready,
    Leased,
    InProgress,
    MergePending,
    Complete,
    Failed,
    Unresolved,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Leased => "leased",
            Self::InProgress => "in_progress",
            Self::MergePending => "merge_pending",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Unresolved => "unresolved",
        }
    }

    pub(super) fn from_db(value: &str) -> Option<Self> {
        match value {
            "ready" => Some(Self::Ready),
            "leased" => Some(Self::Leased),
            "in_progress" => Some(Self::InProgress),
            "merge_pending" => Some(Self::MergePending),
            "complete" => Some(Self::Complete),
            "failed" => Some(Self::Failed),
            "unresolved" => Some(Self::Unresolved),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklogTask {
    pub task_id: String,
    pub kind: TaskKind,
    pub title: String,
    pub details: String,
    pub rationale: String,
    pub scope_key: String,
    pub priority: Priority,
    pub status: TaskStatus,
    pub last_updated: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub source: String,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
    pub attempt_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    pub kind: TaskKind,
    pub title: String,
    pub details: String,
    pub rationale: String,
    pub scope_key: String,
    pub priority: Priority,
    pub source: String,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualTaskInput {
    pub task_id: String,
    pub kind: TaskKind,
    pub title: String,
    pub details: String,
    pub rationale: String,
    pub scope_key: String,
    pub priority: Priority,
    pub status: TaskStatus,
    pub source: String,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskUpdatePatch {
    pub status: Option<TaskStatus>,
    pub rationale: Option<String>,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
    pub clear_lease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMutation {
    pub before: BacklogTask,
    pub after: BacklogTask,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedSeed {
    pub title: String,
    pub details: String,
    pub rejection_reason: String,
    pub domain: String,
}

#[derive(Debug)]
pub(super) enum WriteCmd {
    Upsert {
        task: NewTask,
        now: i64,
        reply: oneshot::Sender<StoreResult<BacklogTask>>,
    },
    InsertManualTask {
        task: ManualTaskInput,
        now: i64,
        reply: oneshot::Sender<StoreResult<BacklogTask>>,
    },
    ClaimNext {
        lease_owner: String,
        lease_expires_at: i64,
        now: i64,
        reply: oneshot::Sender<StoreResult<Option<BacklogTask>>>,
    },
    MarkInProgress {
        task_id: String,
        lease_owner: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    MarkComplete {
        task_id: String,
        lease_owner: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    RecoverStale {
        now: i64,
        reply: oneshot::Sender<StoreResult<usize>>,
    },
    ReleaseLease {
        task_id: String,
        lease_owner: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    MarkUnresolved {
        task_id: String,
        lease_owner: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    SetUnresolvedReady {
        task_id: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    SetUnresolvedMergePending {
        task_id: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    SetMergePendingReady {
        task_id: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    ClearRelatedPr {
        task_id: String,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    MarkMergePending {
        task_id: String,
        lease_owner: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    ClaimMergePending {
        merge_worker_id: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<Option<BacklogTask>>>,
    },
    SetRelatedPr {
        task_id: String,
        pr_number: i64,
        branch: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    PromoteReadyWithPr {
        now: i64,
        reply: oneshot::Sender<StoreResult<usize>>,
    },
    ReopenCompleteToMergePending {
        task_id: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<bool>>,
    },
    UpdateTaskMetadata {
        task_id: String,
        patch: TaskUpdatePatch,
        now: i64,
        reply: oneshot::Sender<StoreResult<TaskMutation>>,
    },
    InsertRejectedSeed {
        title: String,
        details: String,
        rationale: String,
        domain: String,
        priority: String,
        rejection_reason: String,
        now: i64,
        reply: oneshot::Sender<StoreResult<()>>,
    },
}
