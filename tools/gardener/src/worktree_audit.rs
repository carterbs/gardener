use crate::runtime::ProductionProcessRunner;
use crate::worktree::WorktreeClient;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeAuditSummary {
    pub stale_found: usize,
    pub stale_fixed: usize,
}

pub fn reconcile_worktrees() -> WorktreeAuditSummary {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(_) => return WorktreeAuditSummary::default(),
    };
    let runner = ProductionProcessRunner::new();
    let client = WorktreeClient::new(&runner, &cwd);
    let entries = match client.list() {
        Ok(entries) => entries,
        Err(_) => return WorktreeAuditSummary::default(),
    };
    let stale_found = entries.iter().filter(|entry| !entry.path.exists()).count();
    let stale_fixed = if stale_found > 0 && client.prune_orphans().is_ok() {
        stale_found
    } else {
        0
    };
    WorktreeAuditSummary {
        stale_found,
        stale_fixed,
    }
}
