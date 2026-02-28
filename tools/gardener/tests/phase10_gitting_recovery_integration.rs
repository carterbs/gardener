use gardener::config::{AppConfig, GitOutputMode};
use gardener::runtime::{FakeProcessRunner, ProcessOutput};
use gardener::types::{RuntimeScope, WorkerState};
use gardener::worker::execute_task;
use tempfile::tempdir;

fn turn_completed_output(payload: &str) -> ProcessOutput {
    ProcessOutput {
        exit_code: 0,
        stdout: format!("{{\"type\":\"turn.completed\",\"result\":{payload}}}\n"),
        stderr: String::new(),
    }
}

fn git_status_output(stdout: &str) -> ProcessOutput {
    ProcessOutput {
        exit_code: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
    }
}

fn spawned_commands(runner: &FakeProcessRunner) -> Vec<String> {
    runner
        .spawned()
        .into_iter()
        .map(|request| format!("{} {}", request.program, request.args.join(" ")))
        .collect()
}

fn git_status_command_count(runner: &FakeProcessRunner) -> usize {
    runner
        .spawned()
        .into_iter()
        .filter(|request| request.program == "git" && request.args.first() == Some(&"status".to_string()))
        .count()
}

fn gitting_recovery_config() -> (AppConfig, RuntimeScope, FakeProcessRunner, tempfile::TempDir) {
    let temp = tempdir().expect("tempdir");
    let runner = FakeProcessRunner::default();
    let mut cfg = AppConfig::default();
    cfg.execution.test_mode = false;
    cfg.execution.git_output_mode = GitOutputMode::CommitOnly;
    let scope = RuntimeScope {
        process_cwd: temp.path().to_path_buf(),
        repo_root: Some(temp.path().to_path_buf()),
        working_dir: temp.path().to_path_buf(),
    };
    (cfg, scope, runner, temp)
}

#[test]
fn commit_only_gitting_recovery_fails_when_pre_commit_changes_persist() {
    let (cfg, scope, runner, _tmp) = gitting_recovery_config();

    runner.push_response(Ok(git_status_output(
        "worktree /repo\nbranch refs/heads/main\n",
    )));
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
    runner.push_response(Ok(turn_completed_output(
        r#"{"task_type":"task","reasoning":"standard task"}"#,
    )));
    runner.push_response(Ok(turn_completed_output(
        r#"{"summary":"implement commit-only recovery test","files_changed":["README.md"],"commit_message":"test: exercise pre-commit recovery"}"#,
    )));
    runner.push_response(Ok(turn_completed_output(
        r#"{"branch":"gardener/commit-only-recovery","pr_number":12,"pr_url":"https://example.test/pr/12"}"#,
    )));
    runner.push_response(Ok(git_status_output("M src/main.rs\n")));
    runner.push_response(Ok(turn_completed_output(
        r#"{"branch":"gardener/commit-only-recovery","pr_number":12,"pr_url":"https://example.test/pr/12"}"#,
    )));
    runner.push_response(Ok(git_status_output("M src/main.rs\n")));

    let summary = execute_task(
        &cfg,
        &runner,
        &scope,
        "worker-10",
        "task-git-recovery-failed",
        "fix: add recovery assertion coverage",
        1,
    )
    .expect("execution should surface failure summary");

    assert_eq!(summary.final_state, WorkerState::Failed);
    let reason = summary
        .failure_reason
        .expect("failure reason should be present when worktree dirty after recovery");
    assert!(
        reason.contains("pre-commit recovery attempt"),
        "unexpected failure reason: {reason}"
    );
    assert_eq!(
        git_status_command_count(&runner),
        2,
    );
}

#[test]
fn commit_only_gitting_recovery_recovers_when_second_status_is_clean() {
    let (cfg, scope, runner, _tmp) = gitting_recovery_config();

    runner.push_response(Ok(git_status_output(
        "worktree /repo\nbranch refs/heads/main\n",
    )));
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
    runner.push_response(Ok(turn_completed_output(
        r#"{"task_type":"task","reasoning":"standard task"}"#,
    )));
    runner.push_response(Ok(turn_completed_output(
        r#"{"summary":"implement commit-only recovery test","files_changed":["README.md"],"commit_message":"test: exercise pre-commit recovery"}"#,
    )));
    runner.push_response(Ok(turn_completed_output(
        r#"{"branch":"gardener/commit-only-recovery","pr_number":12,"pr_url":"https://example.test/pr/12"}"#,
    )));
    runner.push_response(Ok(git_status_output("M src/main.rs\n")));
    runner.push_response(Ok(turn_completed_output(
        r#"{"branch":"gardener/commit-only-recovery","pr_number":12,"pr_url":"https://example.test/pr/12"}"#,
    )));
    runner.push_response(Ok(git_status_output("")));
    runner.push_response(Ok(turn_completed_output(
        r#"{"verdict":"approve","suggestions":[]}"#,
    )));
    runner.push_response(Ok(git_status_output("")));
    runner.push_response(Ok(turn_completed_output(
        r#"{"merged":true,"merge_sha":"beadbeef"}"#,
    )));
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }));

    let summary = execute_task(
        &cfg,
        &runner,
        &scope,
        "worker-10",
        "task-git-recovery-success",
        "fix: add recovery assertion coverage",
        1,
    )
    .expect("execution should complete");

    if summary.final_state != WorkerState::Complete {
        let reason = summary.failure_reason.unwrap_or_else(|| "no failure reason".to_string());
        panic!(
            "expected recovery to succeed, got {reason}\ncommands:\n{}",
            spawned_commands(&runner).join("\n")
        );
    }
    assert!(summary.failure_reason.is_none());
    let teardown = summary.teardown.expect("complete state should include teardown");
    assert!(teardown.merge_verified);
    assert!(teardown.worktree_cleaned);
}
