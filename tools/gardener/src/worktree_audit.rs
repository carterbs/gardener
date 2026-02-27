#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeAuditSummary {
    pub stale_found: usize,
    pub stale_fixed: usize,
}

pub fn reconcile_worktrees() -> WorktreeAuditSummary {
    WorktreeAuditSummary::default()
}
