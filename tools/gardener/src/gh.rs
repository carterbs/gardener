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

impl MergeStateStatus {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Unknown | Self::Blocked)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrMergeability {
    pub mergeable: Mergeable,
    #[serde(rename = "mergeStateStatus")]
    pub merge_state_status: MergeStateStatus,
}

#[derive(Debug, Clone, Deserialize)]
struct PrCheck {
    bucket: String,
    link: String,
    name: String,
}

#[derive(Debug, Clone)]
pub struct FailedCheck {
    pub name: String,
    pub link: String,
    pub log_snippet: String,
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
            if let Some(url) = existing_pr_url_from_stderr(&out.stderr) {
                if let Some(number) = parse_pr_number_from_url(&url) {
                    append_run_log(
                        "info",
                        "gh.pr.create.already_exists",
                        json!({
                            "cwd": self.cwd.display().to_string(),
                            "title": title,
                            "pr_number": number,
                            "pr_url": url
                        }),
                    );
                    return Ok((number, url));
                }
            }
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
        let number = parse_pr_number_from_url(&url).ok_or_else(|| {
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
            if m.mergeable != Mergeable::Unknown && !m.merge_state_status.is_pending() {
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

    pub fn fetch_failed_checks(&self, pr_number: u64) -> Result<Vec<FailedCheck>, GardenerError> {
        append_run_log(
            "info",
            "gh.pr.checks.fetch_failed",
            json!({ "cwd": self.cwd.display().to_string(), "pr_number": pr_number }),
        );
        let out = self.runner.run(ProcessRequest {
            program: "gh".to_string(),
            args: vec![
                "pr".to_string(),
                "checks".to_string(),
                pr_number.to_string(),
                "--json".to_string(),
                "name,state,bucket,link".to_string(),
            ],
            cwd: Some(self.cwd.clone()),
        })?;
        // exit code 8 = checks still pending, not a failure
        if out.exit_code != 0 && out.exit_code != 8 {
            return Err(GardenerError::Process(format!(
                "gh pr checks failed: {}",
                out.stderr
            )));
        }
        if out.exit_code == 8 || out.stdout.trim().is_empty() {
            return Ok(vec![]);
        }
        let checks: Vec<PrCheck> = serde_json::from_str(&out.stdout)
            .map_err(|e| GardenerError::Process(format!("invalid gh pr checks json: {e}")))?;

        let mut failed = Vec::new();
        for check in checks.iter().filter(|c| c.bucket == "fail") {
            let log_snippet = if let Some(run_id) = extract_run_id_from_link(&check.link) {
                let log_out = self.runner.run(ProcessRequest {
                    program: "gh".to_string(),
                    args: vec![
                        "run".to_string(),
                        "view".to_string(),
                        run_id.to_string(),
                        "--log-failed".to_string(),
                    ],
                    cwd: Some(self.cwd.clone()),
                })?;
                if log_out.exit_code == 0 {
                    truncate_log(&log_out.stdout, 150)
                } else {
                    format!("(failed to fetch logs: {})", log_out.stderr.trim())
                }
            } else {
                "(could not extract run ID from link)".to_string()
            };
            failed.push(FailedCheck {
                name: check.name.clone(),
                link: check.link.clone(),
                log_snippet,
            });
        }
        append_run_log(
            "info",
            "gh.pr.checks.fetch_failed.done",
            json!({ "pr_number": pr_number, "failed_count": failed.len() }),
        );
        Ok(failed)
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

fn extract_run_id_from_link(link: &str) -> Option<u64> {
    // link format: https://github.com/owner/repo/actions/runs/{run_id}/job/{job_id}
    let runs_idx = link.find("/runs/")?;
    let after_runs = &link[runs_idx + "/runs/".len()..];
    let id_str = after_runs.split('/').next()?;
    id_str.parse::<u64>().ok()
}

fn truncate_log(log: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = log.lines().collect();
    if lines.len() <= max_lines {
        log.to_string()
    } else {
        let start = lines.len() - max_lines;
        lines[start..].join("\n")
    }
}

fn parse_pr_number_from_url(url: &str) -> Option<u64> {
    url.rsplit('/').next().and_then(|s| s.parse::<u64>().ok())
}

fn existing_pr_url_from_stderr(stderr: &str) -> Option<String> {
    if !stderr.contains("already exists") {
        return None;
    }
    stderr
        .lines()
        .find(|line| line.contains("http") && line.contains("/pull/"))
        .map(|line| line.trim().to_string())
}

pub fn generate_pr_title_body(
    runner: &dyn ProcessRunner,
    cwd: &Path,
    task_summary: &str,
) -> Result<(String, String), GardenerError> {
    // Fetch full commit messages separated by NUL bytes.
    let log_out = runner.run(ProcessRequest {
        program: "git".to_string(),
        args: vec![
            "log".to_string(),
            "main..HEAD".to_string(),
            "--reverse".to_string(),
            "--format=%B%x00".to_string(),
        ],
        cwd: Some(cwd.to_path_buf()),
    })?;

    let commits: Vec<(&str, &str)> = log_out
        .stdout
        .split('\0')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|msg| {
            // Split each commit message into subject (first line) and body (rest).
            match msg.split_once('\n') {
                Some((subj, rest)) => (subj.trim(), rest.trim()),
                None => (msg, ""),
            }
        })
        .collect();

    let title = commits
        .first()
        .map(|(subj, _)| *subj)
        .filter(|subj| is_good_pr_title(subj))
        .map(|s| s.to_string())
        .unwrap_or_else(|| pr_title_from_summary(task_summary));

    let body = if commits.len() == 1 {
        let (_, desc) = commits[0];
        if desc.is_empty() {
            task_summary.to_string()
        } else {
            desc.to_string()
        }
    } else if commits.is_empty() {
        task_summary.to_string()
    } else {
        let entries: Vec<String> = commits
            .iter()
            .map(|(subj, desc)| {
                if desc.is_empty() {
                    format!("- {subj}")
                } else {
                    format!("- {subj}\n\n  {desc}")
                }
            })
            .collect();
        format!("{task_summary}\n\n## Commits\n\n{}", entries.join("\n"))
    };

    Ok((title, body))
}

fn is_good_pr_title(title: &str) -> bool {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return false;
    }
    let normalized = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    let lowered = normalized.to_ascii_lowercase();
    if matches!(
        lowered.as_str(),
        "feat: implement task changes"
            | "implement task changes"
            | "wip"
            | "update code"
            | "misc changes"
            | "fix stuff"
    ) {
        return false;
    }
    // Must be conventional-commit format: <type>: <description>
    match normalized.split_once(':') {
        Some((kind, desc)) => !kind.trim().is_empty() && !desc.trim().is_empty(),
        None => false,
    }
}

fn pr_title_from_summary(task_summary: &str) -> String {
    let first_line = task_summary.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return "feat: implement requested changes".to_string();
    }
    let normalized = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let max_desc_len = 72usize.saturating_sub("feat: ".len());
    let desc: String = normalized.chars().take(max_desc_len).collect();
    if desc.is_empty() {
        "feat: implement requested changes".to_string()
    } else {
        format!("feat: {desc}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        extract_run_id_from_link, generate_pr_title_body, is_good_pr_title, pr_title_from_summary,
        truncate_log, upgrade_unmerged_collision_priority, GhClient, MergeStateStatus, Mergeable,
        PrMergeability,
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
    fn create_pr_reuses_existing_pr_when_gh_reports_already_exists() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "a pull request for branch \"feat/x\" into branch \"main\" already exists:\nhttps://github.com/owner/repo/pull/18\n".to_string(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let (number, url) = gh.create_pr("title", "body").expect("should reuse");
        assert_eq!(number, 18);
        assert_eq!(url, "https://github.com/owner/repo/pull/18");
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
    fn generate_pr_title_body_single_commit_no_description() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "feat: add widget\n\0".to_string(),
            stderr: String::new(),
        }));
        let (title, body) =
            generate_pr_title_body(&runner, std::path::Path::new("/repo"), "add a widget")
                .expect("ok");
        assert_eq!(title, "feat: add widget");
        assert_eq!(body, "add a widget");
    }

    #[test]
    fn generate_pr_title_body_single_commit_with_description() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "feat: add widget\n\nAdds the widget component with tests.\n\0".to_string(),
            stderr: String::new(),
        }));
        let (title, body) =
            generate_pr_title_body(&runner, std::path::Path::new("/repo"), "add a widget")
                .expect("ok");
        assert_eq!(title, "feat: add widget");
        assert_eq!(body, "Adds the widget component with tests.");
    }

    #[test]
    fn generate_pr_title_body_multiple_commits() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "feat: first\n\nFirst description.\n\0fix: second\n\0".to_string(),
            stderr: String::new(),
        }));
        let (title, body) =
            generate_pr_title_body(&runner, std::path::Path::new("/repo"), "my task summary")
                .expect("ok");
        assert_eq!(title, "feat: first");
        assert!(body.contains("my task summary"));
        assert!(body.contains("- feat: first"));
        assert!(body.contains("First description."));
        assert!(body.contains("- fix: second"));
    }

    #[test]
    fn mergeability_enum_deserializes_from_gh_json() {
        let json = r#"{"mergeable":"MERGEABLE","mergeStateStatus":"BEHIND"}"#;
        let m: PrMergeability = serde_json::from_str(json).expect("parse");
        assert_eq!(m.mergeable, Mergeable::Mergeable);
        assert_eq!(m.merge_state_status, MergeStateStatus::Behind);
    }

    #[test]
    fn is_good_pr_title_accepts_conventional_commit() {
        assert!(is_good_pr_title("feat: add widget"));
        assert!(is_good_pr_title("fix(worker): correct state transition"));
        assert!(is_good_pr_title("chore: bump dependencies"));
    }

    #[test]
    fn is_good_pr_title_rejects_blocklisted_and_invalid() {
        assert!(!is_good_pr_title(""));
        assert!(!is_good_pr_title("feat: implement task changes"));
        assert!(!is_good_pr_title("wip"));
        assert!(!is_good_pr_title("update code"));
        assert!(!is_good_pr_title("misc changes"));
        assert!(!is_good_pr_title("fix stuff"));
        // Non-conventional-commit format
        assert!(!is_good_pr_title("just some words"));
    }

    #[test]
    fn pr_title_from_summary_truncates_to_72_chars() {
        let long = "a]".repeat(50);
        let title = pr_title_from_summary(&long);
        assert!(title.len() <= 72);
        assert!(title.starts_with("feat: "));
    }

    #[test]
    fn is_pending_returns_true_for_unknown_and_blocked() {
        assert!(MergeStateStatus::Unknown.is_pending());
        assert!(MergeStateStatus::Blocked.is_pending());
    }

    #[test]
    fn is_pending_returns_false_for_terminal_states() {
        assert!(!MergeStateStatus::Clean.is_pending());
        assert!(!MergeStateStatus::Dirty.is_pending());
        assert!(!MergeStateStatus::Unstable.is_pending());
        assert!(!MergeStateStatus::Behind.is_pending());
        assert!(!MergeStateStatus::HasHooks.is_pending());
    }

    #[test]
    fn poll_mergeability_waits_through_blocked() {
        let runner = FakeProcessRunner::default();
        // First poll: Mergeable but Blocked (CI still running)
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED"}"#.to_string(),
            stderr: String::new(),
        }));
        // Second poll: resolved to Clean
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN"}"#.to_string(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let m = gh
            .poll_mergeability(5, 3, Duration::from_millis(1))
            .expect("ok");
        assert_eq!(m.merge_state_status, MergeStateStatus::Clean);
        // Should have made 2 calls (waited through Blocked)
        assert_eq!(runner.spawned().len(), 2);
    }

    #[test]
    fn extract_run_id_from_link_parses_github_actions_url() {
        let link = "https://github.com/owner/repo/actions/runs/12345/job/67890";
        assert_eq!(extract_run_id_from_link(link), Some(12345));
    }

    #[test]
    fn extract_run_id_from_link_returns_none_for_bad_url() {
        assert_eq!(extract_run_id_from_link("https://example.com"), None);
        assert_eq!(extract_run_id_from_link(""), None);
    }

    #[test]
    fn truncate_log_keeps_last_n_lines() {
        let log = (1..=200).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let truncated = truncate_log(&log, 150);
        let lines: Vec<&str> = truncated.lines().collect();
        assert_eq!(lines.len(), 150);
        assert_eq!(lines[0], "line 51");
        assert_eq!(lines[149], "line 200");
    }

    #[test]
    fn truncate_log_returns_full_when_under_limit() {
        let log = "line 1\nline 2\nline 3";
        assert_eq!(truncate_log(log, 150), log);
    }

    #[test]
    fn fetch_failed_checks_handles_no_failures() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"[{"bucket":"pass","link":"https://github.com/o/r/actions/runs/1/job/2","name":"test","state":"SUCCESS"}]"#.to_string(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let failed = gh.fetch_failed_checks(42).expect("ok");
        assert!(failed.is_empty());
    }

    #[test]
    fn fetch_failed_checks_returns_failed_with_logs() {
        let runner = FakeProcessRunner::default();
        // gh pr checks response
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"[{"bucket":"fail","link":"https://github.com/o/r/actions/runs/999/job/1","name":"validate","state":"FAILURE"}]"#.to_string(),
            stderr: String::new(),
        }));
        // gh run view --log-failed response
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "error: test failed\nassert_eq failed".to_string(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let failed = gh.fetch_failed_checks(42).expect("ok");
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].name, "validate");
        assert!(failed[0].log_snippet.contains("error: test failed"));
    }

    #[test]
    fn fetch_failed_checks_handles_pending_exit_code_8() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 8,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let gh = GhClient::new(&runner, "/repo");
        let failed = gh.fetch_failed_checks(42).expect("ok");
        assert!(failed.is_empty());
    }

    #[test]
    fn generate_pr_title_body_falls_back_when_first_commit_is_generic() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "feat: implement task changes\n\0".to_string(),
            stderr: String::new(),
        }));
        let (title, _body) =
            generate_pr_title_body(&runner, std::path::Path::new("/repo"), "enable clippy lint")
                .expect("ok");
        assert_eq!(title, "feat: enable clippy lint");
    }
}
