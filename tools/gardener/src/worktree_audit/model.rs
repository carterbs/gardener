use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedWorktree {
    pub path: PathBuf,
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeAuditInspection {
    pub total_worktrees: usize,
    pub stale_found: usize,
}

impl WorktreeAuditInspection {
    pub fn should_prune(&self) -> bool {
        self.stale_found > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeAuditSummary {
    pub stale_found: usize,
    pub stale_fixed: usize,
}
