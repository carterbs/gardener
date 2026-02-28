use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::runtime::{ProcessRequest, ProcessRunner};
use serde_json::json;
use std::fs;
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
        if let Some(existing_entry) = existing.iter().find(|entry| entry.path == path) {
            if existing_entry.branch.as_deref() == Some(branch) {
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
                "error",
                "worktree.create.path_collision",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "path": path.display().to_string(),
                    "requested_branch": branch,
                    "existing_branch": existing_entry.branch,
                }),
            );
            return Err(GardenerError::Process(
                "worktree create failed: path is already used by another worktree".to_string(),
            ));
        }

        if path.exists() {
            if Self::is_empty_directory(path) {
                if let Err(error) = fs::remove_dir_all(path) {
                    append_run_log(
                        "error",
                        "worktree.create.preexisting_path_remove_failed",
                        json!({
                            "cwd": self.cwd.display().to_string(),
                            "path": path.display().to_string(),
                            "error": error.to_string()
                        }),
                    );
                    return Err(GardenerError::Process(
                        "worktree create failed: unable to clear stale path".to_string(),
                    ));
                }
                append_run_log(
                    "warn",
                    "worktree.create.preexisting_path_cleaned",
                    json!({
                        "cwd": self.cwd.display().to_string(),
                        "path": path.display().to_string()
                    }),
                );
            } else {
                append_run_log(
                    "error",
                    "worktree.create.preexisting_path_blocked",
                    json!({
                        "cwd": self.cwd.display().to_string(),
                        "path": path.display().to_string()
                    }),
                );
                return Err(GardenerError::Process(
                    "worktree create failed: path exists and is not registered as a git worktree".to_string(),
                ));
            }
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
            if self.branch_exists(branch)? {
                append_run_log(
                    "warn",
                    "worktree.create.branch_already_exists",
                    json!({
                        "cwd": self.cwd.display().to_string(),
                        "path": path.display().to_string(),
                        "branch": branch,
                        "stderr": out.stderr
                    }),
                );
                let attach = self.runner.run(ProcessRequest {
                    program: "git".to_string(),
                    args: vec![
                        "worktree".to_string(),
                        "add".to_string(),
                        path.display().to_string(),
                        branch.to_string(),
                    ],
                    cwd: Some(self.cwd.clone()),
                })?;
                if attach.exit_code == 0 {
                    append_run_log(
                        "info",
                        "worktree.created_from_existing_branch",
                        json!({
                            "cwd": self.cwd.display().to_string(),
                            "path": path.display().to_string(),
                            "branch": branch
                        }),
                    );
                    return Ok(());
                }
                append_run_log(
                    "error",
                    "worktree.create.from_existing_branch_failed",
                    json!({
                        "cwd": self.cwd.display().to_string(),
                        "path": path.display().to_string(),
                        "branch": branch,
                        "exit_code": attach.exit_code,
                        "stderr": attach.stderr
                    }),
                );
            }
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

    fn is_empty_directory(path: &Path) -> bool {
        match fs::read_dir(path) {
            Ok(mut entries) => entries.next().is_none(),
            Err(_) => false,
        }
    }

    fn branch_exists(&self, branch: &str) -> Result<bool, GardenerError> {
        let check = self.runner.run(ProcessRequest {
            program: "git".to_string(),
            args: vec![
                "branch".to_string(),
                "--list".to_string(),
                branch.to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        Ok(check.exit_code == 0 && !check.stdout.trim().is_empty())
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
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

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
    fn create_or_resume_reuses_existing_branch_if_create_fails() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "worktree /repo\nbranch refs/heads/main\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 255,
            stdout: String::new(),
            stderr:
                "fatal: a branch named 'task-1' already exists\n".to_string(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "  task-1\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, "/repo")
            .create_or_resume(Path::new("/repo/.worktrees/task-1"), "task-1")
            .expect("reused branch");

        let spawned = runner.spawned();
        assert_eq!(spawned.len(), 4);
        assert_eq!(spawned[1].args[0], "worktree");
        assert_eq!(spawned[1].args[1], "add");
        assert_eq!(spawned[1].args[3], "-b");
        assert_eq!(spawned[3].args[0], "worktree");
        assert_eq!(spawned[3].args[1], "add");
        assert_eq!(spawned[3].args[3], "task-1");
    }

    #[test]
    fn create_or_resume_cleans_empty_unregistered_path_before_creating() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join(".worktrees/task-1");
        fs::create_dir_all(&path).expect("create stale path");

        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "worktree /repo\nbranch refs/heads/main\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, tmp.path())
            .create_or_resume(&path, "task-1")
            .expect("recreated after cleaning");

        assert!(!path.exists());
        assert_eq!(runner.spawned().len(), 2);
    }

    #[test]
    fn create_or_resume_fails_on_non_empty_unregistered_path() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join(".worktrees/task-1");
        fs::create_dir_all(&path).expect("create stale path");
        fs::write(path.join("leftover.txt"), "leftover").expect("create stale file");

        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "worktree /repo\nbranch refs/heads/main\n".to_string(),
            stderr: String::new(),
        }));

        let err = WorktreeClient::new(&runner, tmp.path())
            .create_or_resume(&path, "task-1")
            .expect_err("non-empty stale path blocked");

        assert!(
            err.to_string().contains("not registered as a git worktree"),
            "unexpected error: {err}"
        );
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
