use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::runtime::{ProcessRequest, ProcessRunner};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeMode {
    MergeableOnly,
    MergeToMain,
}

pub struct GitClient<'a> {
    runner: &'a dyn ProcessRunner,
    cwd: PathBuf,
}

impl<'a> GitClient<'a> {
    pub fn new(runner: &'a dyn ProcessRunner, cwd: impl AsRef<Path>) -> Self {
        Self {
            runner,
            cwd: cwd.as_ref().to_path_buf(),
        }
    }

    pub fn commit_all(&self, message: &str) -> Result<(), GardenerError> {
        if self.worktree_is_clean()? {
            append_run_log(
                "info",
                "git.commit.skipped_clean",
                json!({ "cwd": self.cwd.display().to_string() }),
            );
            return Ok(());
        }
        append_run_log(
            "info",
            "git.commit.started",
            json!({ "cwd": self.cwd.display().to_string(), "message": message }),
        );
        let add = self.run(["git", "add", "-A"])?;
        if add.exit_code != 0 {
            append_run_log(
                "error",
                "git.commit.add_failed",
                json!({ "cwd": self.cwd.display().to_string(), "stderr": add.stderr }),
            );
            return Err(GardenerError::Process(format!(
                "git add -A failed: {}",
                add.stderr
            )));
        }
        let commit = self.run(["git", "commit", "-m", message])?;
        if commit.exit_code != 0 {
            append_run_log(
                "error",
                "git.commit.failed",
                json!({ "cwd": self.cwd.display().to_string(), "stderr": commit.stderr }),
            );
            return Err(GardenerError::Process(format!(
                "git commit failed: {}",
                commit.stderr
            )));
        }
        append_run_log(
            "info",
            "git.commit.succeeded",
            json!({ "cwd": self.cwd.display().to_string(), "message": message }),
        );
        Ok(())
    }

    pub fn worktree_is_clean(&self) -> Result<bool, GardenerError> {
        let out = self.run(["git", "status", "--porcelain"])?;
        let clean = out.exit_code == 0 && out.stdout.trim().is_empty();
        append_run_log(
            "debug",
            "git.worktree.clean_check",
            json!({
                "cwd": self.cwd.display().to_string(),
                "clean": clean,
                "exit_code": out.exit_code,
                "dirty_lines": out.stdout.lines().count()
            }),
        );
        Ok(clean)
    }

    pub fn detect_detached_head(&self) -> Result<bool, GardenerError> {
        let out = self.run(["git", "symbolic-ref", "--short", "HEAD"])?;
        let detached = out.exit_code != 0;
        append_run_log(
            "debug",
            "git.head.checked",
            json!({
                "cwd": self.cwd.display().to_string(),
                "detached": detached,
                "exit_code": out.exit_code
            }),
        );
        Ok(detached)
    }

    pub fn verify_ancestor(
        &self,
        maybe_ancestor: &str,
        branch: &str,
    ) -> Result<bool, GardenerError> {
        let out = self.run(["git", "merge-base", "--is-ancestor", maybe_ancestor, branch])?;
        let is_ancestor = out.exit_code == 0;
        append_run_log(
            "debug",
            "git.ancestor.verified",
            json!({
                "cwd": self.cwd.display().to_string(),
                "maybe_ancestor": maybe_ancestor,
                "branch": branch,
                "is_ancestor": is_ancestor,
                "exit_code": out.exit_code
            }),
        );
        Ok(is_ancestor)
    }

    pub fn push_with_rebase_recovery(&self, branch: &str) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "git.push.started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "branch": branch
            }),
        );
        let first = self.run(["git", "push", "origin", branch])?;
        if first.exit_code == 0 {
            append_run_log(
                "info",
                "git.push.succeeded",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "branch": branch,
                    "attempt": 1
                }),
            );
            return Ok(());
        }

        append_run_log(
            "warn",
            "git.push.failed_first_attempt",
            json!({
                "cwd": self.cwd.display().to_string(),
                "branch": branch,
                "exit_code": first.exit_code,
                "stderr": first.stderr
            }),
        );

        let rebase = self.run(["git", "pull", "--rebase", "origin", branch])?;
        if rebase.exit_code != 0 {
            append_run_log(
                "error",
                "git.push.rebase_failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "branch": branch,
                    "exit_code": rebase.exit_code,
                    "stderr": rebase.stderr
                }),
            );
            return Err(GardenerError::Process(
                "push/rebase recovery failed".to_string(),
            ));
        }

        append_run_log(
            "info",
            "git.push.rebase_succeeded",
            json!({
                "cwd": self.cwd.display().to_string(),
                "branch": branch
            }),
        );

        let second = self.run(["git", "push", "origin", branch])?;
        if second.exit_code != 0 {
            append_run_log(
                "error",
                "git.push.failed_after_rebase",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "branch": branch,
                    "exit_code": second.exit_code,
                    "stderr": second.stderr
                }),
            );
            return Err(GardenerError::Process(
                "push failed after rebase recovery".to_string(),
            ));
        }

        append_run_log(
            "info",
            "git.push.succeeded",
            json!({
                "cwd": self.cwd.display().to_string(),
                "branch": branch,
                "attempt": 2
            }),
        );
        Ok(())
    }

    pub fn rebase_onto_main(&self, base_branch: &str) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "git.rebase.started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "base_branch": base_branch
            }),
        );
        let fetch = self.run(["git", "fetch", "origin", base_branch])?;
        if fetch.exit_code != 0 {
            append_run_log(
                "warn",
                "git.rebase.fetch_failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "base_branch": base_branch,
                    "stderr": fetch.stderr
                }),
            );
            return Err(GardenerError::Process(format!(
                "git fetch origin {base_branch} failed: {}",
                fetch.stderr
            )));
        }
        let rebase_ref = format!("origin/{base_branch}");
        let rebase = self.run(["git", "rebase", &rebase_ref])?;
        if rebase.exit_code != 0 {
            append_run_log(
                "warn",
                "git.rebase.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "base_branch": base_branch,
                    "stderr": rebase.stderr
                }),
            );
            let _ = self.run(["git", "rebase", "--abort"]);
            return Err(GardenerError::Process(format!(
                "rebase onto origin/{base_branch} failed: {}",
                rebase.stderr
            )));
        }
        append_run_log(
            "info",
            "git.rebase.succeeded",
            json!({
                "cwd": self.cwd.display().to_string(),
                "base_branch": base_branch
            }),
        );
        Ok(())
    }

    pub fn run_validation_command(&self, command: &str) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "git.validation.started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "command": command
            }),
        );
        let out = self.run(["sh", "-lc", command])?;
        if out.exit_code != 0 {
            append_run_log(
                "error",
                "git.validation.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "command": command,
                    "exit_code": out.exit_code,
                    "stderr": out.stderr
                }),
            );
            return Err(GardenerError::Process(
                "post-merge validation command failed".to_string(),
            ));
        }
        append_run_log(
            "info",
            "git.validation.passed",
            json!({
                "cwd": self.cwd.display().to_string(),
                "command": command
            }),
        );
        Ok(())
    }

    fn run<I, S>(&self, args: I) -> Result<crate::runtime::ProcessOutput, GardenerError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let parts = args
            .into_iter()
            .map(|s| s.as_ref().to_string())
            .collect::<Vec<_>>();
        append_run_log(
            "debug",
            "git.run.requested",
            json!({
                "cwd": self.cwd.display().to_string(),
                "program": parts.first().cloned().unwrap_or_default(),
                "arg_count": parts.len().saturating_sub(1),
            }),
        );
        let program = parts.first().cloned().unwrap_or_default();
        let argv = parts.iter().skip(1).cloned().collect::<Vec<_>>();
        self.runner.run(ProcessRequest {
            program,
            args: argv,
            cwd: Some(self.cwd.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::GitClient;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};

    #[test]
    fn push_rebase_recovery_path() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "push failed".to_string(),
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

        GitClient::new(&runner, "/repo")
            .push_with_rebase_recovery("feature/x")
            .expect("recovered");
    }

    #[test]
    fn rebase_onto_main_succeeds() {
        let runner = FakeProcessRunner::default();
        // fetch
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // rebase
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        GitClient::new(&runner, "/repo")
            .rebase_onto_main("main")
            .expect("rebased");
        let spawned = runner.spawned();
        assert!(spawned[0].args.contains(&"fetch".to_string()));
        assert!(spawned[1].args.contains(&"rebase".to_string()));
    }

    #[test]
    fn rebase_onto_main_aborts_on_conflict() {
        let runner = FakeProcessRunner::default();
        // fetch succeeds
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // rebase fails
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "conflict".to_string(),
        }));
        // rebase --abort
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let err = GitClient::new(&runner, "/repo")
            .rebase_onto_main("main")
            .unwrap_err();
        assert!(err.to_string().contains("rebase onto origin/main failed"));
        let spawned = runner.spawned();
        assert!(spawned[2].args.contains(&"--abort".to_string()));
    }

    #[test]
    fn worktree_clean_when_status_empty() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        assert!(GitClient::new(&runner, "/repo")
            .worktree_is_clean()
            .unwrap());
    }

    #[test]
    fn worktree_dirty_when_status_has_output() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: " M tools/gardener/src/tui.rs\n".to_string(),
            stderr: String::new(),
        }));
        assert!(!GitClient::new(&runner, "/repo")
            .worktree_is_clean()
            .unwrap());
    }

    #[test]
    fn detached_head_detection() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let detached = GitClient::new(&runner, "/repo")
            .detect_detached_head()
            .expect("checked");
        assert!(detached);
    }
}
