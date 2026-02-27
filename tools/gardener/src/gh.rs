use crate::errors::GardenerError;
use crate::git::{GitClient, MergeMode};
use crate::logging::append_run_log;
use crate::priority::Priority;
use crate::runtime::{ProcessRequest, ProcessRunner};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct PrView {
    #[serde(rename = "mergedAt")]
    pub merged_at: Option<String>,
    #[serde(rename = "mergeCommit")]
    pub merge_commit: Option<MergeCommit>,
    #[serde(rename = "headRefName")]
    pub head_ref_name: String,
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MergeCommit {
    pub oid: String,
}

pub struct GhClient<'a> {
    runner: &'a dyn ProcessRunner,
    cwd: PathBuf,
}

impl<'a> GhClient<'a> {
    pub fn new(runner: &'a dyn ProcessRunner, cwd: impl AsRef<Path>) -> Self {
        Self {
            runner,
            cwd: cwd.as_ref().to_path_buf(),
        }
    }

    pub fn view_pr(&self, pr_number: u64) -> Result<PrView, GardenerError> {
        append_run_log(
            "info",
            "gh.pr.view.started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "pr_number": pr_number
            }),
        );
        let out = self.runner.run(ProcessRequest {
            program: "gh".to_string(),
            args: vec![
                "pr".to_string(),
                "view".to_string(),
                pr_number.to_string(),
                "--json".to_string(),
                "mergedAt,mergeCommit,headRefName,state".to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if out.exit_code != 0 {
            append_run_log(
                "error",
                "gh.pr.view.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "pr_number": pr_number,
                    "exit_code": out.exit_code,
                    "stderr": out.stderr
                }),
            );
            return Err(GardenerError::Process(format!(
                "gh pr view failed: {}",
                out.stderr
            )));
        }
        let pr: PrView = serde_json::from_str(&out.stdout)
            .map_err(|e| GardenerError::Process(format!("invalid gh pr view json: {e}")))?;
        append_run_log(
            "info",
            "gh.pr.view.fetched",
            json!({
                "cwd": self.cwd.display().to_string(),
                "pr_number": pr_number,
                "state": pr.state,
                "head_ref_name": pr.head_ref_name,
                "merged_at": pr.merged_at,
                "merge_commit_oid": pr.merge_commit.as_ref().map(|c| c.oid.as_str())
            }),
        );
        Ok(pr)
    }

    pub fn verify_merged_and_validated(
        &self,
        git: &GitClient,
        pr_number: u64,
        merge_mode: MergeMode,
        validation_command: &str,
    ) -> Result<String, GardenerError> {
        append_run_log(
            "info",
            "gh.pr.verify.started",
            json!({
                "cwd": self.cwd.display().to_string(),
                "pr_number": pr_number,
                "merge_mode": format!("{:?}", merge_mode),
                "validation_command": validation_command
            }),
        );
        let pr = self.view_pr(pr_number)?;
        let is_merged = pr.state.eq_ignore_ascii_case("merged") || pr.merged_at.is_some();
        if !is_merged {
            append_run_log(
                "warn",
                "gh.pr.verify.not_merged",
                json!({
                    "pr_number": pr_number,
                    "state": pr.state,
                    "merged_at": pr.merged_at
                }),
            );
            return Err(GardenerError::Process(
                "pr is not merged; deterministic escalation required".to_string(),
            ));
        }
        let merge_sha = pr
            .merge_commit
            .as_ref()
            .map(|c| c.oid.clone())
            .ok_or_else(|| {
                append_run_log(
                    "error",
                    "gh.pr.verify.missing_merge_commit",
                    json!({
                        "pr_number": pr_number,
                        "state": pr.state
                    }),
                );
                GardenerError::Process("merged pr missing merge commit".to_string())
            })?;

        if merge_mode == MergeMode::MergeToMain && !git.verify_ancestor(&merge_sha, "main")? {
            append_run_log(
                "error",
                "gh.pr.verify.not_ancestor_of_main",
                json!({
                    "pr_number": pr_number,
                    "merge_sha": merge_sha
                }),
            );
            return Err(GardenerError::Process(
                "merge commit is not an ancestor of main".to_string(),
            ));
        }

        git.run_validation_command(validation_command)?;
        append_run_log(
            "info",
            "gh.pr.verify.succeeded",
            json!({
                "pr_number": pr_number,
                "merge_sha": merge_sha,
                "merge_mode": format!("{:?}", merge_mode)
            }),
        );
        Ok(merge_sha)
    }
}

pub fn upgrade_unmerged_collision_priority(existing: Priority) -> Priority {
    match existing {
        Priority::P0 => Priority::P0,
        Priority::P1 => Priority::P0,
        Priority::P2 => Priority::P1,
    }
}

#[cfg(test)]
mod tests {
    use super::{upgrade_unmerged_collision_priority, GhClient};
    use crate::git::{GitClient, MergeMode};
    use crate::priority::Priority;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};

    #[test]
    fn merged_verification_requires_merged_state_and_validation() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"mergedAt\":\"2026-01-01\",\"mergeCommit\":{\"oid\":\"abc\"},\"headRefName\":\"feat/x\",\"state\":\"MERGED\"}".to_string(),
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

        let gh = GhClient::new(&runner, "/repo");
        let git = GitClient::new(&runner, "/repo");
        let sha = gh
            .verify_merged_and_validated(&git, 123, MergeMode::MergeToMain, "npm run validate")
            .expect("verified");
        assert_eq!(sha, "abc");
    }

    #[test]
    fn unmerged_collision_priority_escalates_one_level() {
        assert_eq!(
            upgrade_unmerged_collision_priority(Priority::P0),
            Priority::P0
        );
        assert_eq!(
            upgrade_unmerged_collision_priority(Priority::P1),
            Priority::P0
        );
        assert_eq!(
            upgrade_unmerged_collision_priority(Priority::P2),
            Priority::P1
        );
    }
}
