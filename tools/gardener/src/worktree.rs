use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::runtime::{ProcessRequest, ProcessRunner};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub detached: bool,
}

pub struct WorktreeClient<'a> {
    runner: &'a dyn ProcessRunner,
    cwd: PathBuf,
}

impl<'a> WorktreeClient<'a> {
    pub fn new(runner: &'a dyn ProcessRunner, cwd: impl AsRef<Path>) -> Self {
        Self {
            runner,
            cwd: cwd.as_ref().to_path_buf(),
        }
    }

    pub fn list(&self) -> Result<Vec<WorktreeEntry>, GardenerError> {
        let out = self.runner.run(ProcessRequest {
            program: "git".to_string(),
            args: vec![
                "worktree".to_string(),
                "list".to_string(),
                "--porcelain".to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if out.exit_code != 0 {
            append_run_log(
                "error",
                "worktree.list.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "exit_code": out.exit_code,
                    "stderr": out.stderr
                }),
            );
            return Err(GardenerError::Process(
                "git worktree list failed".to_string(),
            ));
        }
        let entries = parse_porcelain(&out.stdout)?;
        append_run_log(
            "debug",
            "worktree.list.fetched",
            json!({
                "cwd": self.cwd.display().to_string(),
                "count": entries.len()
            }),
        );
        Ok(entries)
    }

    pub fn create_or_resume(&self, path: &Path, branch: &str) -> Result<(), GardenerError> {
        let existing = self.list()?;
        if existing
            .iter()
            .any(|entry| entry.path == path && entry.branch.as_deref() == Some(branch))
        {
            append_run_log(
                "debug",
                "worktree.create.resumed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "path": path.display().to_string(),
                    "branch": branch
                }),
            );
            return Ok(());
        }

        append_run_log(
            "info",
            "worktree.create.started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "path": path.display().to_string(),
                "branch": branch
            }),
        );
        let out = self.runner.run(ProcessRequest {
            program: "git".to_string(),
            args: vec![
                "worktree".to_string(),
                "add".to_string(),
                path.display().to_string(),
                "-b".to_string(),
                branch.to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if out.exit_code != 0 {
            append_run_log(
                "error",
                "worktree.create.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "path": path.display().to_string(),
                    "branch": branch,
                    "exit_code": out.exit_code,
                    "stderr": out.stderr
                }),
            );
            return Err(GardenerError::Process("worktree create failed".to_string()));
        }
        append_run_log(
            "info",
            "worktree.created",
            json!({
                "cwd": self.cwd.display().to_string(),
                "path": path.display().to_string(),
                "branch": branch
            }),
        );
        Ok(())
    }

    pub fn remove_recreate_if_stale_empty(
        &self,
        path: &Path,
        branch: &str,
    ) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "worktree.stale.remove_started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "path": path.display().to_string(),
                "branch": branch
            }),
        );
        let remove = self.runner.run(ProcessRequest {
            program: "git".to_string(),
            args: vec![
                "worktree".to_string(),
                "remove".to_string(),
                "--force".to_string(),
                path.display().to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if remove.exit_code != 0 {
            append_run_log(
                "error",
                "worktree.stale.remove_failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "path": path.display().to_string(),
                    "branch": branch,
                    "exit_code": remove.exit_code,
                    "stderr": remove.stderr
                }),
            );
            return Err(GardenerError::Process("worktree remove failed".to_string()));
        }
        append_run_log(
            "info",
            "worktree.stale.removed",
            json!({
                "cwd": self.cwd.display().to_string(),
                "path": path.display().to_string(),
                "branch": branch
            }),
        );
        self.create_or_resume(path, branch)
    }

    pub fn cleanup_on_completion(&self, path: &Path) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "worktree.cleanup.started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "path": path.display().to_string()
            }),
        );
        let out = self.runner.run(ProcessRequest {
            program: "git".to_string(),
            args: vec![
                "worktree".to_string(),
                "remove".to_string(),
                "--force".to_string(),
                path.display().to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if out.exit_code != 0 {
            append_run_log(
                "error",
                "worktree.cleanup.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "path": path.display().to_string(),
                    "exit_code": out.exit_code,
                    "stderr": out.stderr
                }),
            );
            return Err(GardenerError::Process(
                "worktree cleanup failed".to_string(),
            ));
        }
        append_run_log(
            "info",
            "worktree.cleaned_up",
            json!({
                "cwd": self.cwd.display().to_string(),
                "path": path.display().to_string()
            }),
        );
        Ok(())
    }

    pub fn prune_orphans(&self) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "worktree.prune.started",
            json!({
                "cwd": self.cwd.display().to_string()
            }),
        );
        let out = self.runner.run(ProcessRequest {
            program: "git".to_string(),
            args: vec!["worktree".to_string(), "prune".to_string()],
            cwd: Some(self.cwd.clone()),
        })?;
        if out.exit_code != 0 {
            append_run_log(
                "error",
                "worktree.prune.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "exit_code": out.exit_code,
                    "stderr": out.stderr
                }),
            );
            return Err(GardenerError::Process("worktree prune failed".to_string()));
        }
        append_run_log(
            "info",
            "worktree.pruned",
            json!({
                "cwd": self.cwd.display().to_string()
            }),
        );
        Ok(())
    }
}

fn parse_porcelain(text: &str) -> Result<Vec<WorktreeEntry>, GardenerError> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut current_detached = false;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(path) = current_path.take() {
                entries.push(WorktreeEntry {
                    path,
                    branch: current_branch.take(),
                    detached: current_detached,
                });
                current_detached = false;
            }
            current_path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(rest.to_string());
        } else if line == "detached" {
            current_detached = true;
        }
    }

    if let Some(path) = current_path {
        entries.push(WorktreeEntry {
            path,
            branch: current_branch,
            detached: current_detached,
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::WorktreeClient;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use std::path::Path;

    #[test]
    fn create_or_resume_is_idempotent() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "worktree /repo\nbranch refs/heads/main\n\nworktree /repo/.worktrees/task-1\nbranch refs/heads/task-1\n".to_string(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, "/repo")
            .create_or_resume(Path::new("/repo/.worktrees/task-1"), "task-1")
            .expect("idempotent resume");
        assert_eq!(runner.spawned().len(), 1);
    }

    #[test]
    fn prune_and_cleanup_paths_run() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));

        let client = WorktreeClient::new(&runner, "/repo");
        client.prune_orphans().expect("pruned");
        client
            .cleanup_on_completion(Path::new("/repo/.worktrees/task-1"))
            .expect("cleanup");
    }
}
