use crate::agent::factory::AdapterFactory;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::fsm::MergingOutput;
use crate::gh::GhClient;
use crate::git::GitClient;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::merge_loop::{run_merge_loop, MergeLoopContext, MergeLoopOutcome};
use crate::prompt_registry::PromptRegistry;
use crate::runtime::{Clock, FileSystem, ProcessRunner};
use crate::types::{RuntimeScope, WorkerActivityState, WorkerState};
use crate::worker::stream_events::{
    emit_adapter_tool_event, emit_worker_activity_state, emit_worker_activity_state_with,
    emit_worker_tool_command,
};
use crate::worker::types::{MergeRequest, TeardownReport, WorkerRunSummary, WorkerStreamEvent};
use crate::worker_identity::WorkerIdentity;
use crate::worktree::WorktreeClient;

/// Execute the merge-and-teardown phase for a task that passed review.
/// Called by the merge worker thread — no mutex needed since the merge worker
/// is single-threaded by construction.
pub(crate) fn execute_merge_phase(
    req: &MergeRequest,
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    runtime_file_system: &dyn FileSystem,
    runtime_clock: &dyn Clock,
    scope: &RuntimeScope,
    on_event: Option<&dyn Fn(WorkerStreamEvent)>,
) -> Result<WorkerRunSummary, GardenerError> {
    let worker_id = &req.worker_id;
    let task_id = &req.task_id;

    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Merging, on_event);

    let factory = AdapterFactory::with_defaults();
    let registry = PromptRegistry::v1();
    let mut learning_loop = LearningLoop::default();
    let identity = WorkerIdentity::new(worker_id);
    let gh = GhClient::new(process_runner, &req.worktree_path);
    let git = GitClient::new(process_runner, &req.worktree_path);
    let repo_root_git = GitClient::new(process_runner, &scope.working_dir);
    let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);
    let worktree_client = WorktreeClient::new(process_runner, repo_root);

    let logs = req.logs.clone();

    // Build the pre-merge validation closure.
    // In the Clean|HasHooks arm the merge loop checks required_checks_green
    // to decide whether to skip validation; for the worker path we always
    // supply the closure and let the merge loop's own skip-logic in the
    // `on_step` callback handle the gating.
    let pre_merge_validation = || -> Result<(), GardenerError> {
        // Check if we can skip validation because required checks are green.
        let skip = match gh.required_checks_green(req.pr_number) {
            Ok(true) => {
                append_run_log(
                    "info",
                    "worker.merging.pre_validation.skipped",
                    serde_json::json!({
                        "worker_id": worker_id,
                        "pr_number": req.pr_number,
                        "reason": "mergeable_clean_and_required_checks_green"
                    }),
                );
                true
            }
            Ok(false) => {
                append_run_log(
                    "debug",
                    "worker.merging.pre_validation.required_checks_not_green",
                    serde_json::json!({
                        "worker_id": worker_id,
                        "pr_number": req.pr_number
                    }),
                );
                false
            }
            Err(err) => {
                append_run_log(
                    "warn",
                    "worker.merging.pre_validation.gate_check_failed",
                    serde_json::json!({
                        "worker_id": worker_id,
                        "pr_number": req.pr_number,
                        "error": err.to_string()
                    }),
                );
                false
            }
        };
        if skip {
            return Ok(());
        }
        emit_worker_tool_command(
            task_id,
            on_event,
            &format!("{} (pre-merge validation)", cfg.validation.command),
        );
        run_repo_validation_with_quality_guard(
            &repo_root_git,
            runtime_file_system,
            runtime_clock,
            cfg,
            scope,
        )
    };

    // Map step labels to WorkerActivityState events.
    let on_step = |label: &str, detail: &str| {
        match label {
            "POLL" => {
                // Parse out attempt and status info from detail for the payload.
                emit_worker_activity_state_with(
                    worker_id,
                    task_id,
                    WorkerActivityState::MergePolling,
                    serde_json::json!({ "detail": detail }),
                    on_event,
                );
            }
            "VALIDATE" => {
                emit_worker_tool_command(task_id, on_event, detail);
            }
            "MERGE" => {
                emit_worker_tool_command(task_id, on_event, detail);
            }
            "REMEDIATE" => {
                // Determine the right activity state from the detail string.
                let state = if detail.contains("CI") || detail.contains("blocked with failed") {
                    WorkerActivityState::CiFailureRemediation
                } else if detail.contains("behind main") || detail.contains("merge from main") {
                    WorkerActivityState::MergeFromMain
                } else {
                    WorkerActivityState::MergeRemediation
                };
                emit_worker_activity_state_with(
                    worker_id,
                    task_id,
                    state,
                    serde_json::json!({ "detail": detail }),
                    on_event,
                );
            }
            _ => {}
        }
    };

    let on_adapter_event = |agent_event: &crate::protocol::AgentEvent| {
        emit_adapter_tool_event(task_id, on_event, agent_event);
    };

    let merge_outcome = run_merge_loop(&mut MergeLoopContext {
        cfg,
        process_runner,
        scope,
        worktree_path: &req.worktree_path,
        factory: &factory,
        registry: &registry,
        learning_loop: &mut learning_loop,
        identity: &identity,
        task_summary: &req.task_summary,
        attempt_count: req.attempt_count,
        gh: &gh,
        git: &git,
        branch: &req.branch,
        pr_number: req.pr_number,
        validation_command: &cfg.validation.command,
        pre_merge_validation: Some(&pre_merge_validation),
        on_step: Some(&on_step),
        on_agent_event: Some(&on_adapter_event),
    })?;

    // Handle non-merged outcomes.
    let merge_sha = match merge_outcome {
        MergeLoopOutcome::Merged { sha } => sha,
        MergeLoopOutcome::Parked { reason } => {
            emit_worker_activity_state(
                worker_id,
                task_id,
                WorkerActivityState::Parked,
                on_event,
            );
            return Ok(WorkerRunSummary {
                worker_id: req.worker_id.clone(),
                session_id: req.session_id.clone(),
                final_state: WorkerState::Parked,
                logs,
                teardown: None,
                failure_reason: Some(reason),
            });
        }
        MergeLoopOutcome::Failed { reason } => {
            emit_worker_activity_state(
                worker_id,
                task_id,
                WorkerActivityState::Failed,
                on_event,
            );
            return Ok(WorkerRunSummary {
                worker_id: req.worker_id.clone(),
                session_id: req.session_id.clone(),
                final_state: WorkerState::Failed,
                logs,
                teardown: None,
                failure_reason: Some(reason),
            });
        }
    };

    let merge_output = MergingOutput {
        merged: true,
        merge_sha: Some(merge_sha.clone()),
    };

    // Post-merge validation
    emit_worker_activity_state(
        worker_id,
        task_id,
        WorkerActivityState::PostMergeValidation,
        on_event,
    );
    emit_worker_tool_command(
        task_id,
        on_event,
        &format!("{} (post-merge validation)", cfg.validation.command),
    );
    if let Err(err) = run_repo_validation_with_quality_guard(
        &repo_root_git,
        runtime_file_system,
        runtime_clock,
        cfg,
        scope,
    ) {
        append_run_log(
            "warn",
            "worker.recovery.post_merge_validation_failed_but_merged",
            serde_json::json!({
                "worker_id": worker_id,
                "task_id": task_id,
                "error": err.to_string(),
                "merge_sha": merge_output.merge_sha
            }),
        );
    }

    // Friction analysis
    {
        let fa_run_id = crate::logging::current_run_id().unwrap_or_default();
        let fa_log_path = crate::logging::current_run_log_path()
            .unwrap_or_else(|| scope.working_dir.join(".gardener/otel-logs.jsonl"));
        let fa_input = crate::friction_analysis::FrictionAnalysisInput {
            worker_id,
            task_id,
            task_summary: &req.task_summary,
            merge_sha: merge_output.merge_sha.as_deref(),
            run_id: &fa_run_id,
            log_path: &fa_log_path,
        };
        match crate::friction_analysis::run_friction_analysis(&fa_input, cfg, process_runner, scope)
        {
            Ok(crate::friction_analysis::FrictionAnalysisOutcome::Completed {
                findings,
                smooth_run: _,
            }) if !findings.is_empty() => {
                let db_path = crate::startup::backlog_db_path(cfg, scope);
                if let Ok(store) = crate::backlog_store::BacklogStore::open(db_path) {
                    for task in crate::friction_analysis::findings_to_tasks(&findings) {
                        if let Err(e) = store.upsert_task(task) {
                            append_run_log(
                                "warn",
                                "friction_analysis.backlog_upsert_error",
                                serde_json::json!({
                                    "worker_id": worker_id,
                                    "error": e.to_string()
                                }),
                            );
                        }
                    }
                    append_run_log(
                        "info",
                        "friction_analysis.tasks_created",
                        serde_json::json!({
                            "worker_id": worker_id,
                            "count": findings.len()
                        }),
                    );
                }
            }
            Ok(crate::friction_analysis::FrictionAnalysisOutcome::Completed {
                findings: _,
                smooth_run: _,
            }) => {
                append_run_log(
                    "debug",
                    "friction_analysis.smooth_run",
                    serde_json::json!({ "worker_id": worker_id }),
                );
            }
            Ok(crate::friction_analysis::FrictionAnalysisOutcome::Skipped { reason }) => {
                append_run_log(
                    "debug",
                    "friction_analysis.skipped",
                    serde_json::json!({ "worker_id": worker_id, "reason": reason }),
                );
            }
            Err(e) => {
                append_run_log(
                    "warn",
                    "friction_analysis.error",
                    serde_json::json!({
                        "worker_id": worker_id,
                        "error": e.to_string()
                    }),
                );
            }
        }
    }

    // Teardown
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Teardown, on_event);
    let teardown = teardown_after_completion(
        &worktree_client,
        &req.worktree_path,
        &merge_output,
        &repo_root_git,
        worker_id,
    );
    append_run_log(
        "info",
        "worker.merge_phase.complete",
        serde_json::json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "merge_verified": teardown.merge_verified,
            "worktree_cleaned": teardown.worktree_cleaned,
            "main_updated": teardown.main_updated
        }),
    );
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Complete, on_event);

    Ok(WorkerRunSummary {
        worker_id: req.worker_id.clone(),
        session_id: req.session_id.clone(),
        final_state: WorkerState::Complete,
        logs,
        teardown: Some(teardown),
        failure_reason: None,
    })
}

fn run_repo_validation_with_quality_guard(
    repo_root_git: &GitClient<'_>,
    _runtime_file_system: &dyn FileSystem,
    _runtime_clock: &dyn Clock,
    cfg: &AppConfig,
    _scope: &RuntimeScope,
) -> Result<(), GardenerError> {
    repo_root_git.pull_main().ok();
    repo_root_git.run_validation_command(&cfg.validation.command)
}

fn teardown_after_completion(
    worktree_client: &WorktreeClient<'_>,
    worktree_path: &std::path::Path,
    output: &MergingOutput,
    repo_git: &GitClient<'_>,
    worker_id: &str,
) -> TeardownReport {
    let worktree_cleaned = if output.merged {
        worktree_client.cleanup_on_completion(worktree_path).is_ok()
    } else {
        false
    };
    let main_updated = if output.merged {
        if let Err(err) = repo_git.pull_main_with_stashed_changes() {
            append_run_log(
                "warn",
                "worker.teardown.pull_main_failed",
                serde_json::json!({ "worker_id": worker_id, "error": err.to_string() }),
            );
            false
        } else {
            true
        }
    } else {
        false
    };
    TeardownReport {
        merge_verified: output.merged,
        session_torn_down: output.merged,
        sandbox_torn_down: output.merged,
        worktree_cleaned,
        state_cleared: output.merged,
        main_updated,
    }
}

#[cfg(test)]
mod tests {
    use super::{execute_merge_phase, teardown_after_completion};
    use crate::config::AppConfig;
    use crate::fsm::MergingOutput;
    use crate::git::GitClient;
    use crate::runtime::{FakeProcessRunner, ProcessOutput, ProductionClock, ProductionFileSystem};
    use crate::types::{RuntimeScope, WorkerState};
    use crate::worker::types::MergeRequest;
    use crate::worktree::WorktreeClient;
    use std::path::{Path, PathBuf};

    #[test]
    fn execute_merge_phase_blocks_merge_when_validation_command_fails() {
        let runner = FakeProcessRunner::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let scope = RuntimeScope {
            process_cwd: dir.path().to_path_buf(),
            repo_root: Some(dir.path().to_path_buf()),
            working_dir: dir.path().to_path_buf(),
        };
        let mut cfg = AppConfig::default();
        cfg.validation.command = "npm run validate".to_string();

        // poll_mergeability → MERGEABLE/CLEAN
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN"}"#.to_string(),
            stderr: String::new(),
        }));
        // view_pr → not merged
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergedAt":null,"mergeCommit":null,"headRefName":"gardener/manual-test","state":"OPEN"}"#.to_string(),
            stderr: String::new(),
        }));
        // required_checks_green → false (two responses for the check)
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
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        // pull_main (inside validation)
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // run_validation_command → fails
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "failed validation".to_string(),
        }));

        let req = MergeRequest {
            slot_idx: 0,
            task_id: "manual:test:pre-merge-guard".to_string(),
            task_summary: "test".to_string(),
            attempt_count: 1,
            worker_id: "merge-worker".to_string(),
            session_id: "session-1".to_string(),
            worktree_path: dir.path().join("worktree"),
            branch: "gardener/manual-test".to_string(),
            pr_number: 42,
            logs: Vec::new(),
            handoff_evidence_bundle: None,
        };

        let fs = ProductionFileSystem;
        let clock = ProductionClock;
        let summary = execute_merge_phase(&req, &cfg, &runner, &fs, &clock, &scope, None)
            .expect("merge phase should return summary");
        assert_eq!(summary.final_state, WorkerState::Failed);
        assert!(summary
            .failure_reason
            .as_deref()
            .unwrap_or_default()
            .contains("pre-merge validation failed"));

        let spawned = runner.spawned();
        assert!(!spawned.iter().any(|request| {
            request.program == "gh"
                && request.args.len() >= 2
                && request.args[0] == "pr"
                && request.args[1] == "merge"
        }));
    }

    #[test]
    fn execute_merge_phase_completes_when_post_merge_validation_fails_after_merge() {
        let runner = FakeProcessRunner::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let scope = RuntimeScope {
            process_cwd: dir.path().to_path_buf(),
            repo_root: Some(dir.path().to_path_buf()),
            working_dir: dir.path().to_path_buf(),
        };
        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        cfg.validation.command = "npm run validate".to_string();

        // poll_mergeability → MERGEABLE/CLEAN
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN"}"#.to_string(),
            stderr: String::new(),
        }));
        // view_pr → not merged
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergedAt":null,"mergeCommit":null,"headRefName":"gardener/manual-test","state":"OPEN"}"#.to_string(),
            stderr: String::new(),
        }));
        // required_checks_green → true (skip pre-merge validation)
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"[{"bucket":"PASS"}]"#.to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // merge_pr → success
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // view_pr after merge → merged with sha
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergedAt":"2026-03-06T12:00:00Z","mergeCommit":{"oid":"cafebabe"},"headRefName":"gardener/manual-test","state":"MERGED"}"#.to_string(),
            stderr: String::new(),
        }));
        // post-merge validation: pull_main
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
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
        // post-merge validation: run_validation_command → fails
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "failed validation".to_string(),
        }));
        // teardown: worktree cleanup
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // teardown: pull_main_with_stashed_changes
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
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

        let req = MergeRequest {
            slot_idx: 0,
            task_id: "manual:test:post-merge-validation".to_string(),
            task_summary: "test".to_string(),
            attempt_count: 1,
            worker_id: "merge-worker".to_string(),
            session_id: "session-1".to_string(),
            worktree_path: dir.path().join("worktree"),
            branch: "gardener/manual-test".to_string(),
            pr_number: 42,
            logs: Vec::new(),
            handoff_evidence_bundle: None,
        };

        let fs = ProductionFileSystem;
        let clock = ProductionClock;
        let summary = execute_merge_phase(&req, &cfg, &runner, &fs, &clock, &scope, None)
            .expect("merge phase should return summary");

        assert_eq!(summary.final_state, WorkerState::Complete);
        assert!(summary.failure_reason.is_none());
        assert!(summary
            .teardown
            .as_ref()
            .is_some_and(|t| t.worktree_cleaned));
    }

    #[test]
    fn teardown_after_completion_stashes_dirty_repo_before_main_sync() {
        let runner = FakeProcessRunner::default();
        let worktree_path = PathBuf::from("/repo/.worktrees/task-1");

        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // git status --porcelain
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: " M quality-grades.md\n".to_string(),
            stderr: String::new(),
        }));
        // git config --bool --get core.bare
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        // git rev-parse --is-bare-repository
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        // git stash push -u -m "gardener: runtime main-sync isolation"
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "Saved working tree and index state WIP on main: ...\n".to_string(),
            stderr: String::new(),
        }));
        // git fetch origin main
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // git merge --ff-only origin/main
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // git stash pop --index
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));

        let worktree_client = WorktreeClient::new(&runner, Path::new("/repo"));
        let repo_git = GitClient::new(&runner, "/repo");
        let output = MergingOutput {
            merged: true,
            merge_sha: Some("cafebabe".to_string()),
        };
        let teardown = teardown_after_completion(
            &worktree_client,
            &worktree_path,
            &output,
            &repo_git,
            "worker-1",
        );

        assert!(teardown.worktree_cleaned);
        assert!(teardown.main_updated);
        let spawned = runner.spawned();
        assert_eq!(
            spawned[0].args,
            vec!["worktree", "remove", "--force", "/repo/.worktrees/task-1"]
        );
        assert_eq!(spawned[1].args, vec!["status", "--porcelain"]);
        assert!(spawned.iter().any(|request| request.args
            == vec![
                "stash",
                "push",
                "-u",
                "-m",
                "gardener: runtime main-sync isolation"
            ]));
        assert!(spawned
            .iter()
            .any(|request| request.args == vec!["stash", "pop", "--index"]));
    }

    #[test]
    fn teardown_after_completion_keeps_clean_repo_without_stash() {
        let runner = FakeProcessRunner::default();
        let worktree_path = PathBuf::from("/repo/.worktrees/task-2");

        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // git status --porcelain
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // git config --bool --get core.bare
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        // git rev-parse --is-bare-repository
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "false\n".to_string(),
            stderr: String::new(),
        }));
        // git fetch origin main
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // git merge --ff-only origin/main
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));

        let worktree_client = WorktreeClient::new(&runner, Path::new("/repo"));
        let repo_git = GitClient::new(&runner, "/repo");
        let output = MergingOutput {
            merged: true,
            merge_sha: None,
        };
        let teardown = teardown_after_completion(
            &worktree_client,
            &worktree_path,
            &output,
            &repo_git,
            "worker-2",
        );

        assert!(teardown.main_updated);
        let spawned = runner.spawned();
        assert!(!spawned
            .iter()
            .any(|request| request.args.first() == Some(&"stash".to_string())));
    }
}
