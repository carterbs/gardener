use crate::worktree_audit::model::{
    ObservedWorktree, WorktreeAuditInspection, WorktreeAuditSummary,
};

pub fn inspect_worktrees(entries: &[ObservedWorktree]) -> WorktreeAuditInspection {
    WorktreeAuditInspection {
        total_worktrees: entries.len(),
        stale_found: entries.iter().filter(|entry| !entry.exists).count(),
    }
}

pub fn summarize_reconcile(
    inspection: &WorktreeAuditInspection,
    prune_succeeded: bool,
) -> WorktreeAuditSummary {
    WorktreeAuditSummary {
        stale_found: inspection.stale_found,
        stale_fixed: if prune_succeeded {
            inspection.stale_found
        } else {
            0
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{inspect_worktrees, summarize_reconcile};
    use crate::worktree_audit::model::{ObservedWorktree, WorktreeAuditInspection};
    use std::path::PathBuf;

    #[test]
    fn inspect_worktrees_counts_missing_paths_as_stale() {
        let inspection = inspect_worktrees(&[
            ObservedWorktree {
                path: PathBuf::from("/repo"),
                exists: true,
            },
            ObservedWorktree {
                path: PathBuf::from("/repo/.worktrees/task-1"),
                exists: false,
            },
            ObservedWorktree {
                path: PathBuf::from("/repo/.worktrees/task-2"),
                exists: true,
            },
        ]);

        assert_eq!(
            inspection,
            WorktreeAuditInspection {
                total_worktrees: 3,
                stale_found: 1,
            }
        );
        assert!(inspection.should_prune());
    }

    #[test]
    fn summarize_reconcile_reports_all_stale_paths_when_prune_succeeds() {
        let summary = summarize_reconcile(
            &WorktreeAuditInspection {
                total_worktrees: 4,
                stale_found: 2,
            },
            true,
        );

        assert_eq!(summary.stale_found, 2);
        assert_eq!(summary.stale_fixed, 2);
    }

    #[test]
    fn summarize_reconcile_leaves_stale_fixed_at_zero_when_prune_fails() {
        let summary = summarize_reconcile(
            &WorktreeAuditInspection {
                total_worktrees: 2,
                stale_found: 1,
            },
            false,
        );

        assert_eq!(summary.stale_found, 1);
        assert_eq!(summary.stale_fixed, 0);
    }
}
