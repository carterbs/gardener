use crate::errors::GardenerError;
use crate::git::{GitClient, MergeMode};
use crate::logging::append_run_log;
use crate::priority::Priority;
use crate::runtime::{ProcessRequest, ProcessRunner};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Mergeable {
    Mergeable,
    Conflicting,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MergeStateStatus {
    Clean,
    Dirty,
    Unstable,
    Blocked,
    Behind,
    HasHooks,
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrMergeability {
    pub mergeable: Mergeable,
    #[serde(rename = "mergeStateStatus")]
    pub merge_state_status: MergeStateStatus,
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

    pub fn create_pr(&self, title: &str, body: &str) -> Result<(u64, String), GardenerError> {
        append_run_log(
            "info",
            "gh.pr.create.started",
            json!({ "cwd": self.cwd.display().to_string(), "title": title }),
        );
        let out = self.runner.run(ProcessRequest {
            program: "gh".to_string(),
            args: vec![
                "pr".to_string(),
                "create".to_string(),
                "--title".to_string(),
                title.to_string(),
                "--body".to_string(),
                body.to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if out.exit_code != 0 {
            append_run_log(
                "error",
                "gh.pr.create.failed",
                json!({
                    "cwd": self.cwd.display().to_string(),
                    "title": title,
                    "exit_code": out.exit_code,
                    "stderr": out.stderr
                }),
            );
            return Err(GardenerError::Process(format!(
                "gh pr create failed: {}",
                out.stderr
            )));
        }
        let url = out.stdout.trim().to_string();
        let number = url
            .rsplit('/')
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| {
                GardenerError::Process(format!(
                    "could not parse PR number from gh pr create output: {url}"
                ))
            })?;
        append_run_log(
            "info",
            "gh.pr.create.succeeded",
            json!({
                "cwd": self.cwd.display().to_string(),
                "pr_number": number,
                "pr_url": url
            }),
        );
        Ok((number, url))
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

    pub fn check_mergeability(&self, pr_number: u64) -> Result<PrMergeability, GardenerError> {
        append_run_log(
            "info",
            "gh.pr.mergeability.check",
            json!({ "cwd": self.cwd.display().to_string(), "pr_number": pr_number }),
        );
        let out = self.runner.run(ProcessRequest {
            program: "gh".to_string(),
            args: vec![
                "pr".to_string(),
                "view".to_string(),
                pr_number.to_string(),
                "--json".to_string(),
                "mergeable,mergeStateStatus".to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if out.exit_code != 0 {
            return Err(GardenerError::Process(format!(
                "gh pr view (mergeability) failed: {}",
                out.stderr
            )));
        }
        let m: PrMergeability = serde_json::from_str(&out.stdout)
            .map_err(|e| GardenerError::Process(format!("invalid mergeability json: {e}")))?;
        append_run_log(
            "info",
            "gh.pr.mergeability.result",
            json!({
                "pr_number": pr_number,
                "mergeable": format!("{:?}", m.mergeable),
                "merge_state_status": format!("{:?}", m.merge_state_status)
            }),
        );
        Ok(m)
    }

    pub fn poll_mergeability(
        &self,
        pr_number: u64,
        max_polls: u32,
        interval: Duration,
    ) -> Result<PrMergeability, GardenerError> {
        for attempt in 0..max_polls {
            let m = self.check_mergeability(pr_number)?;
            if m.mergeable != Mergeable::Unknown {
                return Ok(m);
            }
            append_run_log(
                "debug",
                "gh.pr.mergeability.poll_retry",
                json!({
                    "pr_number": pr_number,
                    "attempt": attempt + 1,
                    "max_polls": max_polls
                }),
            );
            if attempt + 1 < max_polls {
                std::thread::sleep(interval);
            }
        }
        // Return the last Unknown result rather than erroring
        self.check_mergeability(pr_number)
    }

    pub fn merge_pr(&self, pr_number: u64) -> Result<(), GardenerError> {
        append_run_log(
            "info",
            "gh.pr.merge.started",
            json!({ "cwd": self.cwd.display().to_string(), "pr_number": pr_number }),
        );
        // Try squash first
        let squash = self.runner.run(ProcessRequest {
            program: "gh".to_string(),
            args: vec![
                "pr".to_string(),
                "merge".to_string(),
                pr_number.to_string(),
                "--squash".to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if squash.exit_code == 0 {
            append_run_log(
                "info",
                "gh.pr.merge.succeeded",
                json!({ "pr_number": pr_number, "strategy": "squash" }),
            );
            return Ok(());
        }
        append_run_log(
            "warn",
            "gh.pr.merge.squash_failed",
            json!({
                "pr_number": pr_number,
                "exit_code": squash.exit_code,
                "stderr": squash.stderr
            }),
        );
        // Fallback to regular merge
        let merge = self.runner.run(ProcessRequest {
            program: "gh".to_string(),
            args: vec![
                "pr".to_string(),
                "merge".to_string(),
                pr_number.to_string(),
                "--merge".to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        if merge.exit_code == 0 {
            append_run_log(
                "info",
                "gh.pr.merge.succeeded",
                json!({ "pr_number": pr_number, "strategy": "merge" }),
            );
            return Ok(());
        }
        append_run_log(
            "error",
            "gh.pr.merge.failed",
            json!({
                "pr_number": pr_number,
                "exit_code": merge.exit_code,
                "stderr": merge.stderr
            }),
        );
        Err(GardenerError::Process(format!(
            "gh pr merge failed (squash then merge): {}",
            merge.stderr
        )))
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

pub fn generate_pr_title_body(
    runner: &dyn ProcessRunner,
    cwd: &Path,
    task_summary: &str,
) -> Result<(String, String), GardenerError> {
    let log_out = runner.run(ProcessRequest {
        program: "git".to_string(),
        args: vec![
            "log".to_string(),
            "main..HEAD".to_string(),
            "--format=%s".to_string(),
        ],
        cwd: Some(cwd.to_path_buf()),
    })?;
    let subjects: Vec<&str> = log_out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    let title = if subjects.len() == 1 {
        subjects[0].to_string()
    } else {
        task_summary.to_string()
    };

    let commit_log = if subjects.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Commits\n\n{}",
            subjects
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let body = format!("{task_summary}{commit_log}");
    Ok((title, body))
}

#[cfg(test)]
mod tests {
    use super::{
        generate_pr_title_body, upgrade_unmerged_collision_priority, GhClient, Mergeable,
        MergeStateStatus, PrMergeability,
    };
    use crate::git::{GitClient, MergeMode};
    use crate::priority::Priority;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use std::time::Duration;

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

    #[test]
    fn create_pr_parses_number_from_url() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "https://github.com/owner/repo/pull/42\n".to_string(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let (number, url) = gh.create_pr("title", "body").expect("ok");
        assert_eq!(number, 42);
        assert_eq!(url, "https://github.com/owner/repo/pull/42");
    }

    #[test]
    fn create_pr_reports_process_error_as_failure() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "creation failed".to_string(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let err = gh.create_pr("title", "body").expect_err("must fail");
        assert!(format!("{err}").contains("gh pr create failed"));
    }

    #[test]
    fn view_pr_invalid_json_reports_parse_error() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "invalid".to_string(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let err = gh.view_pr(5).expect_err("must fail");
        assert!(format!("{err}").contains("invalid gh pr view json"));
    }

    #[test]
    fn verify_merged_requires_pr_merged_state_or_sha() {
        let runner = FakeProcessRunner::default();
        // open PR metadata says open, so merge verification should fail before git checks.
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout:
                "{\"mergedAt\":null,\"mergeCommit\":{\"oid\":\"abc\"},\"headRefName\":\"feat/x\",\"state\":\"OPEN\"}"
                    .to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));

        let gh = GhClient::new(&runner, "/repo");
        let git = GitClient::new(&runner, "/repo");
        let err = gh
            .verify_merged_and_validated(&git, 123, MergeMode::MergeToMain, "npm run validate")
            .expect_err("must fail");
        assert!(format!("{err}").contains("pr is not merged"));
    }

    #[test]
    fn verify_merged_fails_when_merge_commit_missing() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout:
                "{\"mergedAt\":\"2026-01-01\",\"mergeCommit\":null,\"headRefName\":\"feat/x\",\"state\":\"MERGED\"}"
                    .to_string(),
            stderr: String::new(),
        }));
        // merge commit is missing; this should return an error before running git.
        let gh = GhClient::new(&runner, "/repo");
        let git = GitClient::new(&runner, "/repo");
        let err = gh
            .verify_merged_and_validated(&git, 123, MergeMode::MergeToMain, "npm run validate")
            .expect_err("must fail");
        assert!(format!("{err}").contains("merged pr missing merge commit"));
    }

    #[test]
    fn check_mergeability_parses_clean_status() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN"}"#.to_string(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let m = gh.check_mergeability(42).expect("ok");
        assert_eq!(m.mergeable, Mergeable::Mergeable);
        assert_eq!(m.merge_state_status, MergeStateStatus::Clean);
    }

    #[test]
    fn check_mergeability_parses_conflicting_status() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"CONFLICTING","mergeStateStatus":"DIRTY"}"#.to_string(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let m = gh.check_mergeability(10).expect("ok");
        assert_eq!(m.mergeable, Mergeable::Conflicting);
        assert_eq!(m.merge_state_status, MergeStateStatus::Dirty);
    }

    #[test]
    fn poll_mergeability_resolves_after_unknown() {
        let runner = FakeProcessRunner::default();
        // First poll: unknown
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"UNKNOWN","mergeStateStatus":"UNKNOWN"}"#.to_string(),
            stderr: String::new(),
        }));
        // Second poll: resolved
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN"}"#.to_string(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let m = gh
            .poll_mergeability(5, 3, Duration::from_millis(1))
            .expect("ok");
        assert_eq!(m.mergeable, Mergeable::Mergeable);
    }

    #[test]
    fn merge_pr_squash_succeeds() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        gh.merge_pr(42).expect("ok");
        let spawned = runner.spawned();
        assert!(spawned[0].args.contains(&"--squash".to_string()));
    }

    #[test]
    fn merge_pr_falls_back_to_merge_on_squash_failure() {
        let runner = FakeProcessRunner::default();
        // Squash fails
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "squash not allowed".to_string(),
        }));
        // Merge succeeds
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        gh.merge_pr(42).expect("ok");
        let spawned = runner.spawned();
        assert!(spawned[1].args.contains(&"--merge".to_string()));
    }

    #[test]
    fn merge_pr_fails_when_both_strategies_fail() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "squash fail".to_string(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "merge fail".to_string(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let err = gh.merge_pr(42).expect_err("must fail");
        assert!(format!("{err}").contains("gh pr merge failed"));
    }

    #[test]
    fn generate_pr_title_body_single_commit() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "feat: add widget\n".to_string(),
            stderr: String::new(),
        }));
        let (title, body) =
            generate_pr_title_body(&runner, std::path::Path::new("/repo"), "add a widget")
                .expect("ok");
        assert_eq!(title, "feat: add widget");
        assert!(body.contains("add a widget"));
    }

    #[test]
    fn generate_pr_title_body_multiple_commits() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "feat: first\nfix: second\n".to_string(),
            stderr: String::new(),
        }));
        let (title, body) =
            generate_pr_title_body(&runner, std::path::Path::new("/repo"), "my task summary")
                .expect("ok");
        assert_eq!(title, "my task summary");
        assert!(body.contains("- feat: first"));
        assert!(body.contains("- fix: second"));
    }

    #[test]
    fn mergeability_enum_deserializes_from_gh_json() {
        let json = r#"{"mergeable":"MERGEABLE","mergeStateStatus":"BEHIND"}"#;
        let m: PrMergeability = serde_json::from_str(json).expect("parse");
        assert_eq!(m.mergeable, Mergeable::Mergeable);
        assert_eq!(m.merge_state_status, MergeStateStatus::Behind);
    }
}
