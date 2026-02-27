use crate::logging::append_run_log;
use crate::runtime::ProductionProcessRunner;
use crate::worktree::WorktreeClient;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeAuditSummary {
    pub stale_found: usize,
    pub stale_fixed: usize,
}

pub fn reconcile_worktrees() -> WorktreeAuditSummary {
    append_run_log("info", "worktree_audit.started", json!({}));
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            append_run_log(
                "error",
                "worktree_audit.cwd_failed",
                json!({ "error": e.to_string() }),
            );
            return WorktreeAuditSummary::default();
        }
    };
    let runner = ProductionProcessRunner::new();
    let client = WorktreeClient::new(&runner, &cwd);
    let entries = match client.list() {
        Ok(entries) => entries,
        Err(e) => {
            append_run_log(
                "error",
                "worktree_audit.list_failed",
                json!({
                    "cwd": cwd.display().to_string(),
                    "error": e.to_string()
                }),
            );
            return WorktreeAuditSummary::default();
        }
    };
    let total = entries.len();
    let stale_found = entries.iter().filter(|entry| !entry.path.exists()).count();
    append_run_log(
        "info",
        "worktree_audit.inspected",
        json!({
            "cwd": cwd.display().to_string(),
            "total_worktrees": total,
            "stale_found": stale_found
        }),
    );
    let stale_fixed = if stale_found > 0 {
        match client.prune_orphans() {
            Ok(()) => {
                append_run_log(
                    "info",
                    "worktree_audit.pruned",
                    json!({
                        "cwd": cwd.display().to_string(),
                        "stale_fixed": stale_found
                    }),
                );
                stale_found
            }
            Err(e) => {
                append_run_log(
                    "error",
                    "worktree_audit.prune_failed",
                    json!({
                        "cwd": cwd.display().to_string(),
                        "stale_found": stale_found,
                        "error": e.to_string()
                    }),
                );
                0
            }
        }
    } else {
        0
    };
    append_run_log(
        "info",
        "worktree_audit.completed",
        json!({
            "cwd": cwd.display().to_string(),
            "stale_found": stale_found,
            "stale_fixed": stale_fixed
        }),
    );
    WorktreeAuditSummary {
        stale_found,
        stale_fixed,
    }
}
