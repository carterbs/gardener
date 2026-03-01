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
        if let Some(existing_entry) = existing
            .iter()
            .find(|entry| Self::paths_match(&entry.path, path))
        {
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
                self.sync_git_hooks(path);
                return Ok(());
            }

            // Path is registered but branch doesn't match (detached HEAD or
            // leftover from a previous task).  Force-remove and fall through
            // to re-creation instead of returning a fatal error.
            append_run_log(
                "warn",
                "worktree.create.stale_path_reclaim",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "path": path.display().to_string(),
                    "requested_branch": branch,
                    "existing_branch": existing_entry.branch,
                    "detached": existing_entry.detached,
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
                    "worktree.create.stale_path_reclaim_failed",
                    json!({
                        "cwd": self.cwd.display().to_string(),
                        "path": path.display().to_string(),
                        "requested_branch": branch,
                        "existing_branch": existing_entry.branch,
                        "exit_code": remove.exit_code,
                        "stderr": remove.stderr,
                    }),
                );
                return Err(GardenerError::Process(
                    "worktree create failed: path is already used by another worktree and could not be reclaimed".to_string(),
                ));
            }
            append_run_log(
                "info",
                "worktree.create.stale_path_reclaimed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "path": path.display().to_string(),
                    "requested_branch": branch,
                }),
            );
            // Fall through to create the worktree fresh below.
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
                    "worktree create failed: path exists and is not registered as a git worktree"
                        .to_string(),
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
            let branch_exists = self.branch_exists(branch)?;
            let branch_collision = branch_exists
                || out.stderr.to_lowercase().contains("fatal: a branch named")
                    && out.stderr.to_lowercase().contains("already exists");
            if branch_collision {
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
                    self.sync_git_hooks(path);
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
        self.sync_git_hooks(path);
        Ok(())
    }

    fn paths_match(left: &Path, right: &Path) -> bool {
        let left = Self::normalize_path(left);
        let right = Self::normalize_path(right);
        left == right
    }

    fn normalize_path(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn sync_git_hooks(&self, worktree_path: &Path) {
        let source = match self.resolve_hook_path(&self.cwd) {
            Ok(path) => path,
            Err(error) => {
                append_run_log(
                    "debug",
                    "worktree.hooks.path_missing",
                    json!({
                        "cwd": self.cwd.display().to_string(),
                        "error": error.to_string()
                    }),
                );
                return;
            }
        };
        let target = match self.resolve_hook_path(worktree_path) {
            Ok(path) => path,
            Err(error) => {
                append_run_log(
                    "debug",
                    "worktree.hooks.path_missing",
                    json!({
                        "cwd": worktree_path.display().to_string(),
                        "error": error.to_string()
                    }),
                );
                return;
            }
        };
        if let Err(error) = sync_directory(&source, &target) {
            append_run_log(
                "debug",
                "worktree.hooks.sync_failed",
                json!({
                    "source": source.display().to_string(),
                    "target": target.display().to_string(),
                    "error": error.to_string()
                }),
            );
        }
    }

    fn resolve_hook_path(&self, repo_root: &Path) -> Result<PathBuf, GardenerError> {
        append_run_log(
            "debug",
            "worktree.hooks.path_resolve_started",
            json!({ "repo_root": repo_root.display().to_string() }),
        );
        let out = self.runner.run(ProcessRequest {
            program: "git".to_string(),
            args: vec![
                "-C".to_string(),
                repo_root.display().to_string(),
                "rev-parse".to_string(),
                "--git-path".to_string(),
                "hooks".to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if out.exit_code != 0 {
            return Err(GardenerError::Process(format!(
                "failed to resolve hooks path: {}",
                out.stderr
            )));
        }
        let value = out.stdout.trim();
        if value.is_empty() {
            return Err(GardenerError::Process(
                "failed to resolve hooks path: empty output".to_string(),
            ));
        }
        let path = PathBuf::from(value);
        if path.is_absolute() {
            Ok(path)
        } else {
            Ok(repo_root.join(path))
        }
    }

    fn is_empty_directory(path: &Path) -> bool {
        match fs::read_dir(path) {
            Ok(mut entries) => entries.next().is_none(),
            Err(_) => false,
        }
    }

    fn branch_exists(&self, branch: &str) -> Result<bool, GardenerError> {
        append_run_log(
            "debug",
            "worktree.branch_exists.started",
            json!({ "branch": branch }),
        );
        if self.reference_exists(&format!("refs/heads/{branch}"))? {
            return Ok(true);
        }
        if self.reference_exists(&format!("refs/remotes/origin/{branch}"))? {
            return Ok(true);
        }
        if self.reference_exists(&format!("refs/remotes/upstream/{branch}"))? {
            return Ok(true);
        }
        Ok(false)
    }

    fn reference_exists(&self, reference: &str) -> Result<bool, GardenerError> {
        append_run_log(
            "debug",
            "worktree.reference_exists.started",
            json!({ "reference": reference }),
        );
        let check = self.runner.run(ProcessRequest {
            program: "git".to_string(),
            args: vec![
                "show-ref".to_string(),
                "--verify".to_string(),
                "--quiet".to_string(),
                reference.to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        Ok(check.exit_code == 0)
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

fn sync_directory(source: &Path, target: &Path) -> Result<(), GardenerError> {
    let source = fs::canonicalize(source).map_err(|error| GardenerError::Io(error.to_string()))?;
    fs::create_dir_all(target).map_err(|error| GardenerError::Io(error.to_string()))?;

    let mut copied = 0usize;
    for entry in fs::read_dir(&source).map_err(|error| GardenerError::Io(error.to_string()))? {
        let entry = entry.map_err(|error| GardenerError::Io(error.to_string()))?;
        let source_path = entry.path();
        if source_path.is_dir() {
            continue;
        }
        let Some(file_name) = source_path.file_name() else {
            continue;
        };
        let destination_path = target.join(file_name);
        fs::copy(&source_path, &destination_path)
            .map_err(|error| GardenerError::Process(error.to_string()))?;
        copied += 1;
    }

    if copied > 0 {
        append_run_log(
            "debug",
            "worktree.hooks.synced",
            json!({
                "source": source.display().to_string(),
                "target": target.display().to_string(),
                "copied": copied,
            }),
        );
    }

    Ok(())
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
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, "/repo")
            .create_or_resume(Path::new("/repo/.worktrees/task-1"), "task-1")
            .expect("idempotent resume");
        assert_eq!(runner.spawned().len(), 3);
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
            stderr: "fatal: a branch named 'task-1' already exists\n".to_string(),
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
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, "/repo")
            .create_or_resume(Path::new("/repo/.worktrees/task-1"), "task-1")
            .expect("reused branch");

        let spawned = runner.spawned();
        assert_eq!(spawned.len(), 6);
        assert_eq!(spawned[1].args[0], "worktree");
        assert_eq!(spawned[1].args[1], "add");
        assert_eq!(spawned[1].args[3], "-b");
        assert_eq!(spawned[3].args[0], "worktree");
        assert_eq!(spawned[3].args[1], "add");
        assert_eq!(spawned[3].args[3], "task-1");
    }

    #[test]
    fn create_or_resume_reuses_existing_branch_when_branch_exists_reference_check_succeeds() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "worktree /repo\nbranch refs/heads/main\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 255,
            stdout: String::new(),
            stderr: "fatal: unable to write new index file".to_string(),
        }));
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
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, "/repo")
            .create_or_resume(Path::new("/repo/.worktrees/task-1"), "task-1")
            .expect("reused branch via reference check");

        let spawned = runner.spawned();
        assert_eq!(spawned.len(), 6);
        assert_eq!(spawned[1].args[0], "worktree");
        assert_eq!(spawned[1].args[1], "add");
        assert_eq!(spawned[1].args[3], "-b");
        assert_eq!(spawned[2].args[0], "show-ref");
        assert_eq!(spawned[2].args[1], "--verify");
        assert_eq!(spawned[2].args[2], "--quiet");
        assert_eq!(spawned[2].args[3], "refs/heads/task-1");
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
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, tmp.path())
            .create_or_resume(&path, "task-1")
            .expect("recreated after cleaning");

        assert!(!path.exists());
        assert_eq!(runner.spawned().len(), 4);
    }

    #[test]
    fn create_or_resume_copies_repo_hooks_to_worktree() {
        let tmp = tempdir().expect("tempdir");
        let source_hooks = tmp.path().join(".githooks");
        fs::create_dir_all(&source_hooks).expect("create source hooks directory");
        let source_hook = source_hooks.join("pre-commit");
        fs::write(&source_hook, "#!/usr/bin/env bash\necho source\n").expect("write source hook");
        let path = tmp.path().join(".worktrees/task-1");
        fs::create_dir_all(&path).expect("create worktree path");

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
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, tmp.path())
            .create_or_resume(&path, "task-1")
            .expect("hooks copied");

        let copied_hook = path.join(".githooks").join("pre-commit");
        assert!(copied_hook.exists());
        assert_eq!(
            fs::read_to_string(copied_hook).expect("read copied hook"),
            "#!/usr/bin/env bash\necho source\n"
        );
        assert_eq!(runner.spawned().len(), 4);
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
    fn create_or_resume_reclaims_detached_head_worktree() {
        let runner = FakeProcessRunner::default();
        // list returns a worktree at the target path but detached (no branch)
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "worktree /repo\nbranch refs/heads/main\n\nworktree /repo/.worktrees/task-1\ndetached\n".to_string(),
            stderr: String::new(),
        }));
        // git worktree remove --force succeeds
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // git worktree add -b succeeds
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // hook resolve (source)
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));
        // hook resolve (target)
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, "/repo")
            .create_or_resume(Path::new("/repo/.worktrees/task-1"), "task-1")
            .expect("should reclaim detached HEAD worktree");

        let spawned = runner.spawned();
        assert_eq!(spawned.len(), 5);
        // [0] = list, [1] = remove --force, [2] = add -b, [3-4] = hook resolves
        assert_eq!(spawned[1].args[0], "worktree");
        assert_eq!(spawned[1].args[1], "remove");
        assert_eq!(spawned[1].args[2], "--force");
        assert_eq!(spawned[2].args[0], "worktree");
        assert_eq!(spawned[2].args[1], "add");
    }

    #[test]
    fn create_or_resume_reclaims_wrong_branch_worktree() {
        let runner = FakeProcessRunner::default();
        // list returns worktree at target path but on a different branch
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "worktree /repo\nbranch refs/heads/main\n\nworktree /repo/.worktrees/task-1\nbranch refs/heads/old-branch\n".to_string(),
            stderr: String::new(),
        }));
        // git worktree remove --force succeeds
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // git worktree add -b succeeds
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // hook resolves
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: ".githooks\n".to_string(),
            stderr: String::new(),
        }));

        WorktreeClient::new(&runner, "/repo")
            .create_or_resume(Path::new("/repo/.worktrees/task-1"), "task-1")
            .expect("should reclaim wrong-branch worktree");

        let spawned = runner.spawned();
        assert_eq!(spawned.len(), 5);
        assert_eq!(spawned[1].args[1], "remove");
        assert_eq!(spawned[2].args[1], "add");
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
