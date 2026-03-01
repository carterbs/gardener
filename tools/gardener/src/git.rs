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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseResult {
    Clean,
    Conflict { stderr: String },
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

    pub fn head_sha(&self) -> Result<Option<String>, GardenerError> {
        let out = self.run(["git", "rev-parse", "HEAD"])?;
        let sha = if out.exit_code == 0 {
            let s = out.stdout.trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        } else {
            None
        };
        append_run_log(
            "debug",
            "git.head_sha",
            json!({
                "cwd": self.cwd.display().to_string(),
                "sha": sha
            }),
        );
        Ok(sha)
    }

    pub fn commits_since(&self, base_sha: &str) -> Result<Vec<String>, GardenerError> {
        if base_sha.is_empty() {
            return Ok(vec![]);
        }
        let range = format!("{base_sha}..HEAD");
        let out = self.run(["git", "log", &range, "--format=%s"])?;
        let subjects: Vec<String> = if out.exit_code != 0 {
            vec![]
        } else {
            out.stdout
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        };
        append_run_log(
            "debug",
            "git.commits_since",
            json!({
                "cwd": self.cwd.display().to_string(),
                "base_sha": base_sha,
                "count": subjects.len()
            }),
        );
        Ok(subjects)
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

        if !is_non_fast_forward_push(&first.stderr) {
            append_run_log(
                "error",
                "git.push.failed_unrecoverable_first_attempt",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "branch": branch,
                    "exit_code": first.exit_code,
                    "stderr": first.stderr
                }),
            );
            return Err(GardenerError::Process("push failed".to_string()));
        }

        append_run_log(
            "info",
            "git.push.non_fast_forward_detected",
            json!({
                "cwd": self.cwd.display().to_string(),
                "branch": branch
            }),
        );

        let fetch = self.run(["git", "fetch", "origin", branch])?;
        if fetch.exit_code != 0 {
            append_run_log(
                "error",
                "git.push.force_with_lease.fetch_failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "branch": branch,
                    "exit_code": fetch.exit_code,
                    "stderr": fetch.stderr
                }),
            );
            return Err(GardenerError::Process(
                "push/force-with-lease recovery failed".to_string(),
            ));
        }

        let second = self.run(["git", "push", "--force-with-lease", "origin", branch])?;
        if second.exit_code != 0 {
            append_run_log(
                "error",
                "git.push.force_with_lease.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "branch": branch,
                    "exit_code": second.exit_code,
                    "stderr": second.stderr
                }),
            );
            return Err(GardenerError::Process(
                "push/force-with-lease recovery failed".to_string(),
            ));
        }

        append_run_log(
            "info",
            "git.push.force_with_lease.succeeded",
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

    pub fn try_merge_from_main(&self) -> Result<RebaseResult, GardenerError> {
        append_run_log(
            "info",
            "git.merge_from_main.started",
            json!({ "cwd": self.cwd.display().to_string() }),
        );
        let fetch = self.run(["git", "fetch", "origin", "main"])?;
        if fetch.exit_code != 0 {
            append_run_log(
                "error",
                "git.merge_from_main.fetch_failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "stderr": fetch.stderr
                }),
            );
            return Err(GardenerError::Process(format!(
                "git fetch origin main failed: {}",
                fetch.stderr
            )));
        }
        let merge = self.run(["git", "merge", "origin/main", "--no-edit"])?;
        if merge.exit_code == 0 {
            append_run_log(
                "info",
                "git.merge_from_main.clean",
                json!({ "cwd": self.cwd.display().to_string() }),
            );
            return Ok(RebaseResult::Clean);
        }
        let combined = format!("{}\n{}", merge.stdout, merge.stderr);
        if is_merge_conflict(&combined) {
            append_run_log(
                "warn",
                "git.merge_from_main.conflict",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "stdout": merge.stdout,
                    "stderr": merge.stderr
                }),
            );
            // Leave merge in progress — agent resolves markers, commit_all completes it
            return Ok(RebaseResult::Conflict {
                stderr: combined.trim().to_string(),
            });
        }
        // Unknown error — abort the merge and return Err
        append_run_log(
            "error",
            "git.merge_from_main.failed",
            json!({
                "cwd": self.cwd.display().to_string(),
                "exit_code": merge.exit_code,
                "stdout": merge.stdout,
                "stderr": merge.stderr
            }),
        );
        let _ = self.run(["git", "merge", "--abort"]);
        Err(GardenerError::Process(format!(
            "git merge origin/main failed: {}",
            combined.trim()
        )))
    }

    pub fn rebase_onto_local(&self, base: &str) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "git.rebase_local.started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "base": base
            }),
        );
        let rebase = self.run(["git", "rebase", base])?;
        if rebase.exit_code != 0 {
            append_run_log(
                "warn",
                "git.rebase_local.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "base": base,
                    "stderr": rebase.stderr
                }),
            );
            let _ = self.run(["git", "rebase", "--abort"]);
            return Err(GardenerError::Process(format!(
                "rebase onto {base} failed: {}",
                rebase.stderr
            )));
        }
        append_run_log(
            "info",
            "git.rebase_local.succeeded",
            json!({
                "cwd": self.cwd.display().to_string(),
                "base": base
            }),
        );
        Ok(())
    }

    pub fn try_rebase_onto_local(&self, base: &str) -> Result<RebaseResult, GardenerError> {
        append_run_log(
            "info",
            "git.rebase_local.try.started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "base": base
            }),
        );
        let rebase = self.run(["git", "rebase", base])?;
        if rebase.exit_code == 0 {
            append_run_log(
                "info",
                "git.rebase_local.try.succeeded",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "base": base
                }),
            );
            return Ok(RebaseResult::Clean);
        }

        let stderr = rebase.stderr.clone();
        if is_rebase_conflict(&stderr) {
            append_run_log(
                "warn",
                "git.rebase_local.try.conflict",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "base": base,
                    "stderr": stderr,
                }),
            );
            return Ok(RebaseResult::Conflict { stderr });
        }

        append_run_log(
            "warn",
            "git.rebase_local.try.failed",
            json!({
                "cwd": self.cwd.display().to_string(),
                "base": base,
                "exit_code": rebase.exit_code,
                "stderr": stderr
            }),
        );
        Err(GardenerError::Process(format!(
            "rebase onto {base} failed: {stderr}"
        )))
    }

    pub fn abort_rebase(&self) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "git.rebase_local.abort.started",
            json!({
                "cwd": self.cwd.display().to_string()
            }),
        );
        let out = self.run(["git", "rebase", "--abort"])?;
        if out.exit_code != 0 {
            append_run_log(
                "error",
                "git.rebase_local.abort.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "stderr": out.stderr
                }),
            );
            return Err(GardenerError::Process(format!(
                "rebase abort failed: {}",
                out.stderr
            )));
        }
        append_run_log(
            "info",
            "git.rebase_local.abort.succeeded",
            json!({
                "cwd": self.cwd.display().to_string()
            }),
        );
        Ok(())
    }

    pub fn pull_main(&self) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "git.pull_main.started",
            json!({ "cwd": self.cwd.display().to_string() }),
        );
        let fetch = self.run(["git", "fetch", "origin", "main"])?;
        if fetch.exit_code != 0 {
            append_run_log(
                "warn",
                "git.pull_main.fetch_failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "stderr": fetch.stderr
                }),
            );
            return Err(GardenerError::Process(format!(
                "git fetch origin main failed: {}",
                fetch.stderr
            )));
        }
        let merge = self.run(["git", "merge", "--ff-only", "origin/main"])?;
        if merge.exit_code != 0 {
            append_run_log(
                "warn",
                "git.pull_main.merge_failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "stderr": merge.stderr
                }),
            );
            return Err(GardenerError::Process(format!(
                "git merge --ff-only origin/main failed: {}",
                merge.stderr
            )));
        }
        append_run_log(
            "info",
            "git.pull_main.succeeded",
            json!({ "cwd": self.cwd.display().to_string() }),
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

fn is_rebase_conflict(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("conflict") || lower.contains("unmerged files")
}

fn is_merge_conflict(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("conflict") || lower.contains("unmerged files")
}

fn is_non_fast_forward_push(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("non-fast-forward")
        || lower.contains("tip of your current branch is behind")
        || lower.contains("failed to push some refs")
}

#[cfg(test)]
mod tests {
    use super::GitClient;
    use super::RebaseResult;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};

    #[test]
    fn push_force_with_lease_recovery_path() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "non-fast-forward".to_string(),
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
        let spawned = runner.spawned();
        assert_eq!(spawned[1].args, vec!["fetch", "origin", "feature/x"]);
        assert_eq!(
            spawned[2].args,
            vec!["push", "--force-with-lease", "origin", "feature/x"]
        );
    }

    #[test]
    fn push_recovery_bails_on_non_recoverable_error() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "Permission denied (publickey).".to_string(),
        }));
        let err = GitClient::new(&runner, "/repo")
            .push_with_rebase_recovery("feature/x")
            .expect_err("should not attempt recovery");
        assert!(err.to_string().contains("push failed"));
        assert_eq!(runner.spawned().len(), 1);
    }

    #[test]
    fn push_recovery_handles_logged_non_fast_forward_case_without_rebase() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "To github.com:carterbs/gardener.git\n ! [rejected]        gardener/worker-3-manual-quality-e9fccfb4844348f6 -> gardener/worker-3-manual-quality-e9fccfb4844348f6 (non-fast-forward)\nerror: failed to push some refs to 'github.com:carterbs/gardener.git'\nhint: Updates were rejected because the tip of your current branch is behind\nhint: its remote counterpart. If you want to integrate the remote changes,\nhint: use 'git pull' before pushing again.\n".to_string(),
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
            .push_with_rebase_recovery("gardener/worker-3-manual-quality-e9fccfb4844348f6")
            .expect("recovered from logged non-fast-forward");
        let spawned = runner.spawned();
        let joined = spawned
            .iter()
            .flat_map(|cmd| cmd.args.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !joined.contains("pull --rebase"),
            "recovery must not attempt pull --rebase: {joined}"
        );
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
            .expect_err("rebase onto main should fail with simulated conflict");
        assert!(err.to_string().contains("rebase onto origin/main failed"));
        let spawned = runner.spawned();
        assert!(spawned[2].args.contains(&"--abort".to_string()));
    }

    #[test]
    fn try_rebase_onto_local_reports_conflict() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "CONFLICT (content): Merge conflict in src/lib.rs".to_string(),
        }));
        let result = GitClient::new(&runner, "/repo")
            .try_rebase_onto_local("main")
            .expect("try rebase should return conflict");
        match result {
            RebaseResult::Conflict { stderr } => {
                assert!(stderr.contains("CONFLICT"));
            }
            _ => panic!("expected conflict result"),
        }
    }

    #[test]
    fn try_rebase_onto_local_fails_on_unexpected_error() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "could not read ".to_string(),
        }));
        let err = GitClient::new(&runner, "/repo")
            .try_rebase_onto_local("main")
            .expect_err("try rebase should fail");
        assert!(err.to_string().contains("rebase onto main failed"));
    }

    #[test]
    fn abort_rebase_executes_rebase_abort() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        GitClient::new(&runner, "/repo")
            .abort_rebase()
            .expect("abort rebase");
        let spawned = runner.spawned();
        assert!(spawned[0].args.contains(&"--abort".to_string()));
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
            .expect("worktree_is_clean should succeed"));
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
            .expect("worktree_is_clean should succeed"));
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

    #[test]
    fn commits_since_returns_subjects() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "feat: add foo\nfix: bar\n".to_string(),
            stderr: String::new(),
        }));
        let subjects = GitClient::new(&runner, "/repo")
            .commits_since("abc123")
            .expect("commits_since");
        assert_eq!(subjects, vec!["feat: add foo", "fix: bar"]);
        let spawned = runner.spawned();
        assert!(spawned[0].args.contains(&"abc123..HEAD".to_string()));
        assert!(spawned[0].args.contains(&"--format=%s".to_string()));
    }

    #[test]
    fn commits_since_empty_base_returns_empty() {
        let runner = FakeProcessRunner::default();
        let subjects = GitClient::new(&runner, "/repo")
            .commits_since("")
            .expect("empty base");
        assert!(subjects.is_empty());
        assert_eq!(runner.spawned().len(), 0);
    }

    #[test]
    fn commits_since_nonzero_exit_returns_empty() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 128,
            stdout: String::new(),
            stderr: "bad object abc123".to_string(),
        }));
        let subjects = GitClient::new(&runner, "/repo")
            .commits_since("abc123")
            .expect("nonzero returns empty");
        assert!(subjects.is_empty());
    }

    #[test]
    fn verify_ancestor_tracks_expected_results() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        assert!(GitClient::new(&runner, "/repo")
            .verify_ancestor("abc", "main")
            .expect("ancestor"),);

        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "not ancestor".to_string(),
        }));
        assert!(!GitClient::new(&runner, "/repo")
            .verify_ancestor("abc", "main")
            .expect("ancestor"),);
    }

    #[test]
    fn commit_all_runs_add_before_commit() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "M file.txt\n".to_string(),
            stderr: String::new(),
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
            .commit_all("commit changes")
            .expect("commit");
        assert_eq!(runner.spawned().len(), 3);
    }

    #[test]
    fn commit_all_skips_clean_tree() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "\n".to_string(),
            stderr: String::new(),
        }));
        GitClient::new(&runner, "/repo")
            .commit_all("noop")
            .expect("skip clean commit");
        assert_eq!(runner.spawned().len(), 1);
    }

    #[test]
    fn run_validation_command_reports_failure() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "failed validation".to_string(),
        }));
        let err = GitClient::new(&runner, "/repo")
            .run_validation_command("npm run validate")
            .expect_err("validation failed");
        assert!(err
            .to_string()
            .contains("post-merge validation command failed"));
    }

    #[test]
    fn try_merge_from_main_clean() {
        let runner = FakeProcessRunner::default();
        // fetch
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // merge
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let result = GitClient::new(&runner, "/repo")
            .try_merge_from_main()
            .expect("merge from main should succeed");
        assert_eq!(result, RebaseResult::Clean);
        let spawned = runner.spawned();
        assert!(spawned[0].args.contains(&"fetch".to_string()));
        assert!(spawned[1].args.contains(&"merge".to_string()));
        assert!(spawned[1].args.contains(&"origin/main".to_string()));
    }

    #[test]
    fn try_merge_from_main_conflict_leaves_merge_in_progress() {
        let runner = FakeProcessRunner::default();
        // fetch
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // merge fails with conflict
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "CONFLICT (content): Merge conflict in src/lib.rs".to_string(),
        }));
        let result = GitClient::new(&runner, "/repo")
            .try_merge_from_main()
            .expect("should return conflict, not error");
        match result {
            RebaseResult::Conflict { stderr } => {
                assert!(stderr.contains("CONFLICT"));
            }
            _ => panic!("expected conflict result"),
        }
        // Should NOT have called merge --abort
        let spawned = runner.spawned();
        assert_eq!(spawned.len(), 2, "should not abort on conflict");
    }

    #[test]
    fn try_merge_from_main_unknown_error_aborts() {
        let runner = FakeProcessRunner::default();
        // fetch
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // merge fails with non-conflict error
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "fatal: refusing to merge unrelated histories".to_string(),
        }));
        // merge --abort
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let err = GitClient::new(&runner, "/repo")
            .try_merge_from_main()
            .expect_err("should error on unknown failure");
        assert!(err.to_string().contains("git merge origin/main failed"));
        let spawned = runner.spawned();
        assert!(spawned[2].args.contains(&"--abort".to_string()));
    }

    #[test]
    fn rebase_local_recovery_paths_are_exercised() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "oops".to_string(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let err = GitClient::new(&runner, "/repo")
            .rebase_onto_local("main")
            .expect_err("rebase local failed");
        assert!(err.to_string().contains("rebase onto main failed"));
    }
}
