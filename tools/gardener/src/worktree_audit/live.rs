use crate::logging::append_run_log;
use crate::runtime::{ProcessRunner, ProductionProcessRunner};
use crate::worktree::WorktreeClient;
use crate::worktree_audit::logic::{inspect_worktrees, summarize_reconcile};
use crate::worktree_audit::model::{ObservedWorktree, WorktreeAuditSummary};
use serde_json::json;
use std::path::Path;

pub fn reconcile_worktrees() -> WorktreeAuditSummary {
    append_run_log("info", "worktree_audit.started", json!({}));
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            append_run_log(
                "error",
                "worktree_audit.cwd_failed",
                json!({ "error": error.to_string() }),
            );
            return WorktreeAuditSummary::default();
        }
    };

    reconcile_worktrees_in(&cwd)
}

pub fn reconcile_worktrees_in(cwd: &Path) -> WorktreeAuditSummary {
    let runner = ProductionProcessRunner::new();
    reconcile_worktrees_with_runner(cwd, &runner)
}

pub(crate) fn reconcile_worktrees_with_runner(
    cwd: &Path,
    runner: &dyn ProcessRunner,
) -> WorktreeAuditSummary {
    let client = WorktreeClient::new(runner, cwd);
    let entries = match client.list() {
        Ok(entries) => entries,
        Err(error) => {
            append_run_log(
                "error",
                "worktree_audit.list_failed",
                json!({
                    "cwd": cwd.display().to_string(),
                    "error": error.to_string()
                }),
            );
            return WorktreeAuditSummary::default();
        }
    };
    let observed = entries
        .into_iter()
        .map(|entry| ObservedWorktree {
            exists: entry.path.exists(),
            path: entry.path,
        })
        .collect::<Vec<_>>();
    let inspection = inspect_worktrees(&observed);
    append_run_log(
        "info",
        "worktree_audit.inspected",
        json!({
            "cwd": cwd.display().to_string(),
            "total_worktrees": inspection.total_worktrees,
            "stale_found": inspection.stale_found
        }),
    );

    let prune_succeeded = if inspection.should_prune() {
        match client.prune_orphans() {
            Ok(()) => {
                append_run_log(
                    "info",
                    "worktree_audit.pruned",
                    json!({
                        "cwd": cwd.display().to_string(),
                        "stale_fixed": inspection.stale_found
                    }),
                );
                true
            }
            Err(error) => {
                append_run_log(
                    "error",
                    "worktree_audit.prune_failed",
                    json!({
                        "cwd": cwd.display().to_string(),
                        "stale_found": inspection.stale_found,
                        "error": error.to_string()
                    }),
                );
                false
            }
        }
    } else {
        false
    };

    let summary = summarize_reconcile(&inspection, prune_succeeded);
    append_run_log(
        "info",
        "worktree_audit.completed",
        json!({
            "cwd": cwd.display().to_string(),
            "stale_found": summary.stale_found,
            "stale_fixed": summary.stale_fixed
        }),
    );
    summary
}

#[cfg(test)]
mod tests {
    use super::reconcile_worktrees_with_runner;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use tempfile::tempdir;

    #[test]
    fn reconcile_worktrees_with_runner_reports_zero_fixed_when_prune_fails() {
        let temp = tempdir().expect("tempdir");
        let stale_path = temp.path().join(".worktrees/task-1");
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: format!(
                "worktree {}\nbranch refs/heads/main\n\nworktree {}\nbranch refs/heads/task-1\n",
                temp.path().display(),
                stale_path.display()
            ),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "failed".to_string(),
        }));

        let summary = reconcile_worktrees_with_runner(temp.path(), &runner);

        assert_eq!(summary.stale_found, 1);
        assert_eq!(summary.stale_fixed, 0);
    }
}
