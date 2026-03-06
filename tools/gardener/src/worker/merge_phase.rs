use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::fsm::MergingOutput;
use crate::gh::{FailedCheck, GhClient, MergeStateStatus, Mergeable};
use crate::git::{GitClient, RebaseResult};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::{
    ci_failure_remediation_template, merge_main_conflict_resolution_template, PromptRegistry,
};
use crate::protocol::AgentTerminal;
use crate::retry::{retry_with_backoff, RetryConfig};
use crate::runtime::{Clock, FileSystem, ProcessRunner};
use crate::types::{RuntimeScope, WorkerActivityState, WorkerState};
use crate::worker::evidence::log_event_from;
use crate::worker::stream_events::{
    emit_adapter_tool_event, emit_worker_activity_state, emit_worker_activity_state_with,
    emit_worker_tool_command, extract_failure_reason, merge_polling_block_reason,
};
use crate::worker::types::{MergeRequest, TeardownReport, WorkerRunSummary, WorkerStreamEvent};
use crate::worker_identity::WorkerIdentity;
use crate::worktree::WorktreeClient;
use std::time::Duration;

pub const MAX_MERGE_REMEDIATION: u32 = 3;
pub const MERGEABILITY_POLL_MAX: u32 = 12;
pub const MERGEABILITY_POLL_INTERVAL: Duration = Duration::from_secs(5);

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
    let on_adapter_event = |agent_event: &crate::protocol::AgentEvent| {
        emit_adapter_tool_event(task_id, on_event, agent_event);
    };

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

    let pr = req.pr_number;
    let branch = &req.branch;
    let mut logs = req.logs.clone();
    let mut merge_output = MergingOutput {
        merged: false,
        merge_sha: None,
    };

    for attempt in 0..MAX_MERGE_REMEDIATION {
        let status = retry_with_backoff(
            &RetryConfig {
                operation_name: "poll_mergeability",
                max_attempts: 2,
                ..Default::default()
            },
            || gh.poll_mergeability(pr, MERGEABILITY_POLL_MAX, MERGEABILITY_POLL_INTERVAL),
        )?;
        if let Ok(pr_view) = gh.view_pr(pr) {
            if pr_view.state.eq_ignore_ascii_case("MERGED") || pr_view.merged_at.is_some() {
                append_run_log(
                    "info",
                    "worker.merging.already_merged",
                    serde_json::json!({
                        "worker_id": worker_id,
                        "pr_number": pr,
                        "attempt": attempt + 1,
                        "merge_sha": pr_view.merge_commit.as_ref().map(|c| c.oid.clone())
                    }),
                );
                if let Some(sha) = pr_view.merge_commit.as_ref() {
                    merge_output = MergingOutput {
                        merged: true,
                        merge_sha: Some(sha.oid.clone()),
                    };
                } else {
                    merge_output = MergingOutput {
                        merged: true,
                        merge_sha: None,
                    };
                }
                break;
            }
        }

        let block_reason =
            merge_polling_block_reason(&status.mergeable, &status.merge_state_status);
        let mut poll_details = serde_json::json!({
            "pr_number": pr,
            "attempt": attempt + 1,
            "mergeable": format!("{:?}", status.mergeable),
            "merge_state_status": format!("{:?}", status.merge_state_status),
            "next_check_in_secs": MERGEABILITY_POLL_INTERVAL.as_secs()
        });
        if let Some(reason) = block_reason {
            poll_details["block_reason"] = serde_json::json!(reason);
        }
        emit_worker_activity_state_with(
            worker_id,
            task_id,
            WorkerActivityState::MergePolling,
            poll_details,
            on_event,
        );

        append_run_log(
            "info",
            "worker.merging.poll_result",
            serde_json::json!({
                "worker_id": worker_id,
                "pr_number": pr,
                "attempt": attempt + 1,
                "mergeable": format!("{:?}", status.mergeable),
                "merge_state_status": format!("{:?}", status.merge_state_status)
            }),
        );

        match (&status.mergeable, &status.merge_state_status) {
            (Mergeable::Mergeable, MergeStateStatus::Clean | MergeStateStatus::HasHooks) => {
                let skip_pre_validation = match gh.required_checks_green(pr) {
                    Ok(true) => {
                        append_run_log(
                            "info",
                            "worker.merging.pre_validation.skipped",
                            serde_json::json!({
                                "worker_id": worker_id,
                                "pr_number": pr,
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
                                "pr_number": pr
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
                                "pr_number": pr,
                                "error": err.to_string()
                            }),
                        );
                        false
                    }
                };
                if !skip_pre_validation {
                    emit_worker_tool_command(
                        task_id,
                        on_event,
                        &format!("{} (pre-merge validation)", cfg.validation.command),
                    );
                    if let Err(err) = run_repo_validation_with_quality_guard(
                        &repo_root_git,
                        runtime_file_system,
                        runtime_clock,
                        cfg,
                        scope,
                    ) {
                        emit_worker_activity_state(
                            worker_id,
                            task_id,
                            WorkerActivityState::Failed,
                            on_event,
                        );
                        append_run_log(
                            "error",
                            "worker.merging.pre_validation_failed",
                            serde_json::json!({
                                "worker_id": worker_id,
                                "task_id": task_id,
                                "error": err.to_string()
                            }),
                        );
                        return Ok(WorkerRunSummary {
                            worker_id: req.worker_id.clone(),
                            session_id: req.session_id.clone(),
                            final_state: WorkerState::Failed,
                            logs,
                            teardown: None,
                            failure_reason: Some(format!("pre-merge validation failed: {err}")),
                        });
                    }
                }
                emit_worker_tool_command(task_id, on_event, &format!("gh pr merge {pr}"));
                match gh.merge_pr(pr) {
                    Ok(()) => {
                        let view = retry_with_backoff(
                            &RetryConfig {
                                operation_name: "view_pr_after_merge",
                                ..Default::default()
                            },
                            || gh.view_pr(pr),
                        )?;
                        let sha = view.merge_commit.map(|c| c.oid).unwrap_or_default();
                        merge_output = MergingOutput {
                            merged: true,
                            merge_sha: Some(sha.clone()),
                        };
                        append_run_log(
                            "info",
                            "worker.merging.deterministic.succeeded",
                            serde_json::json!({
                                "worker_id": worker_id,
                                "pr_number": pr,
                                "attempt": attempt + 1,
                                "merge_sha": sha
                            }),
                        );
                        break;
                    }
                    Err(merge_err) => {
                        if attempt + 1 >= MAX_MERGE_REMEDIATION {
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
                                failure_reason: Some(format!(
                                    "merge failed after {} attempts: {}",
                                    MAX_MERGE_REMEDIATION, merge_err
                                )),
                            });
                        }
                    }
                }
            }
            (_, MergeStateStatus::Behind) => {
                emit_worker_activity_state_with(
                    worker_id,
                    task_id,
                    WorkerActivityState::MergeFromMain,
                    serde_json::json!({ "pr_number": pr, "attempt": attempt + 1 }),
                    on_event,
                );
                if let Err(e) = worker_merge_main_and_push(
                    &gh,
                    &git,
                    &mut learning_loop,
                    &mut logs,
                    cfg,
                    process_runner,
                    scope,
                    req,
                    &factory,
                    &registry,
                    &identity,
                    pr,
                    branch,
                    worker_id,
                    task_id,
                    attempt,
                    on_event,
                ) {
                    emit_worker_activity_state_with(
                        worker_id,
                        task_id,
                        WorkerActivityState::MergeRemediation,
                        serde_json::json!({ "pr_number": pr, "attempt": attempt + 1 }),
                        on_event,
                    );
                    learning_loop.ingest_failure(
                        WorkerState::Merging,
                        "merge from main failed, agent remediation needed",
                        vec![format!("error={e}")],
                    );
                    let remediation_result = run_agent_turn(AgentTurnInput {
                        cfg,
                        process_runner,
                        scope,
                        worktree_path: &req.worktree_path,
                        factory: &factory,
                        registry: &registry,
                        learning_loop: &learning_loop,
                        identity: &identity,
                        state: WorkerState::Merging,
                        task_summary: &req.task_summary,
                        attempt_count: req.attempt_count,
                        prompt_override: None,
                        on_event: Some(&on_adapter_event),
                    })?;
                    logs.push(log_event_from(&remediation_result, WorkerState::Merging));
                    if remediation_result.terminal == AgentTerminal::Failure
                        && attempt + 1 >= MAX_MERGE_REMEDIATION
                    {
                        emit_worker_activity_state(
                            worker_id,
                            task_id,
                            WorkerActivityState::Failed,
                            on_event,
                        );
                        let failure_reason = extract_failure_reason(&remediation_result.payload);
                        return Ok(WorkerRunSummary {
                            worker_id: req.worker_id.clone(),
                            session_id: req.session_id.clone(),
                            final_state: WorkerState::Failed,
                            logs,
                            teardown: None,
                            failure_reason,
                        });
                    }
                }
                continue;
            }
            (Mergeable::Conflicting, _) | (_, MergeStateStatus::Dirty) => {
                emit_worker_activity_state_with(
                    worker_id,
                    task_id,
                    WorkerActivityState::MergeFromMain,
                    serde_json::json!({ "pr_number": pr, "attempt": attempt + 1 }),
                    on_event,
                );
                if let Err(e) = worker_merge_main_and_push(
                    &gh,
                    &git,
                    &mut learning_loop,
                    &mut logs,
                    cfg,
                    process_runner,
                    scope,
                    req,
                    &factory,
                    &registry,
                    &identity,
                    pr,
                    branch,
                    worker_id,
                    task_id,
                    attempt,
                    on_event,
                ) {
                    emit_worker_activity_state_with(
                        worker_id,
                        task_id,
                        WorkerActivityState::MergeRemediation,
                        serde_json::json!({ "pr_number": pr, "attempt": attempt + 1 }),
                        on_event,
                    );
                    learning_loop.ingest_failure(
                        WorkerState::Merging,
                        "conflict resolution failed, agent remediation needed",
                        vec![format!("error={e}")],
                    );
                    let remediation_result = run_agent_turn(AgentTurnInput {
                        cfg,
                        process_runner,
                        scope,
                        worktree_path: &req.worktree_path,
                        factory: &factory,
                        registry: &registry,
                        learning_loop: &learning_loop,
                        identity: &identity,
                        state: WorkerState::Merging,
                        task_summary: &req.task_summary,
                        attempt_count: req.attempt_count,
                        prompt_override: None,
                        on_event: Some(&on_adapter_event),
                    })?;
                    logs.push(log_event_from(&remediation_result, WorkerState::Merging));
                    if remediation_result.terminal == AgentTerminal::Failure
                        && attempt + 1 >= MAX_MERGE_REMEDIATION
                    {
                        emit_worker_activity_state(
                            worker_id,
                            task_id,
                            WorkerActivityState::Failed,
                            on_event,
                        );
                        let failure_reason = extract_failure_reason(&remediation_result.payload);
                        return Ok(WorkerRunSummary {
                            worker_id: req.worker_id.clone(),
                            session_id: req.session_id.clone(),
                            final_state: WorkerState::Failed,
                            logs,
                            teardown: None,
                            failure_reason,
                        });
                    }
                }
                continue;
            }
            (_, MergeStateStatus::Unstable) => {
                emit_worker_activity_state_with(
                    worker_id,
                    task_id,
                    WorkerActivityState::CiFailureRemediation,
                    serde_json::json!({ "pr_number": pr, "attempt": attempt + 1 }),
                    on_event,
                );
                let failed_checks = gh.fetch_failed_checks(pr).unwrap_or_default();
                let evidence: Vec<String> = failed_checks
                    .iter()
                    .map(|c| {
                        format!(
                            "## CI check: {}\nLink: {}\n\n```\n{}\n```",
                            c.name, c.link, c.log_snippet
                        )
                    })
                    .collect();
                learning_loop.ingest_failure(WorkerState::Merging, "CI checks failed", evidence);
                let ci_tpl = ci_failure_remediation_template();
                let ci_result = run_agent_turn(AgentTurnInput {
                    cfg,
                    process_runner,
                    scope,
                    worktree_path: &req.worktree_path,
                    factory: &factory,
                    registry: &registry,
                    learning_loop: &learning_loop,
                    identity: &identity,
                    state: WorkerState::Merging,
                    task_summary: &req.task_summary,
                    attempt_count: req.attempt_count,
                    prompt_override: Some(&ci_tpl),
                    on_event: Some(&on_adapter_event),
                })?;
                logs.push(log_event_from(&ci_result, WorkerState::Merging));
                if ci_result.terminal == AgentTerminal::Failure
                    && attempt + 1 >= MAX_MERGE_REMEDIATION
                {
                    emit_worker_activity_state(
                        worker_id,
                        task_id,
                        WorkerActivityState::Failed,
                        on_event,
                    );
                    let failure_reason = extract_failure_reason(&ci_result.payload);
                    return Ok(WorkerRunSummary {
                        worker_id: req.worker_id.clone(),
                        session_id: req.session_id.clone(),
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason,
                    });
                }
                continue;
            }
            (_, MergeStateStatus::Blocked) => {
                append_run_log(
                    "warn",
                    "worker.merging.fallback_attempt",
                    serde_json::json!({
                        "worker_id": worker_id,
                        "pr_number": pr,
                        "attempt": attempt + 1,
                        "mergeable": format!("{:?}", status.mergeable),
                        "merge_state_status": format!("{:?}", status.merge_state_status)
                    }),
                );
                let failed_checks = gh.fetch_failed_checks(pr).unwrap_or_default();
                if !has_explicit_failed_checks(&failed_checks) {
                    if attempt + 1 >= MAX_MERGE_REMEDIATION {
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
                            failure_reason: Some(
                                "PR blocked by branch protection rules, parked for retry"
                                    .to_string(),
                            ),
                        });
                    }
                    continue;
                }
                emit_worker_activity_state_with(
                    worker_id,
                    task_id,
                    WorkerActivityState::CiFailureRemediation,
                    serde_json::json!({ "pr_number": pr, "attempt": attempt + 1 }),
                    on_event,
                );
                let evidence: Vec<String> = failed_checks
                    .iter()
                    .map(|c| {
                        format!(
                            "## CI check: {}\nLink: {}\n\n```\n{}\n```",
                            c.name, c.link, c.log_snippet
                        )
                    })
                    .collect();
                learning_loop.ingest_failure(
                    WorkerState::Merging,
                    "required CI checks failed (Blocked)",
                    evidence,
                );
                let ci_tpl = ci_failure_remediation_template();
                let ci_result = run_agent_turn(AgentTurnInput {
                    cfg,
                    process_runner,
                    scope,
                    worktree_path: &req.worktree_path,
                    factory: &factory,
                    registry: &registry,
                    learning_loop: &learning_loop,
                    identity: &identity,
                    state: WorkerState::Merging,
                    task_summary: &req.task_summary,
                    attempt_count: req.attempt_count,
                    prompt_override: Some(&ci_tpl),
                    on_event: Some(&on_adapter_event),
                })?;
                logs.push(log_event_from(&ci_result, WorkerState::Merging));
                if ci_result.terminal == AgentTerminal::Failure
                    && attempt + 1 >= MAX_MERGE_REMEDIATION
                {
                    emit_worker_activity_state(
                        worker_id,
                        task_id,
                        WorkerActivityState::Failed,
                        on_event,
                    );
                    let failure_reason = extract_failure_reason(&ci_result.payload);
                    return Ok(WorkerRunSummary {
                        worker_id: req.worker_id.clone(),
                        session_id: req.session_id.clone(),
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason,
                    });
                }
                continue;
            }
            _ => {
                append_run_log(
                    "warn",
                    "worker.merging.fallback_attempt",
                    serde_json::json!({
                        "worker_id": worker_id,
                        "pr_number": pr,
                        "attempt": attempt + 1,
                        "mergeable": format!("{:?}", status.mergeable),
                        "merge_state_status": format!("{:?}", status.merge_state_status)
                    }),
                );
                if let Err(err) = run_repo_validation_with_quality_guard(
                    &repo_root_git,
                    runtime_file_system,
                    runtime_clock,
                    cfg,
                    scope,
                ) {
                    emit_worker_activity_state(
                        worker_id,
                        task_id,
                        WorkerActivityState::Failed,
                        on_event,
                    );
                    append_run_log(
                        "error",
                        "worker.merging.pre_validation_failed",
                        serde_json::json!({
                            "worker_id": worker_id,
                            "task_id": task_id,
                            "error": err.to_string()
                        }),
                    );
                    return Ok(WorkerRunSummary {
                        worker_id: req.worker_id.clone(),
                        session_id: req.session_id.clone(),
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason: Some(format!("pre-merge validation failed: {err}")),
                    });
                }
                emit_worker_tool_command(
                    task_id,
                    on_event,
                    &format!("{} (pre-merge validation)", cfg.validation.command),
                );
                emit_worker_tool_command(task_id, on_event, &format!("gh pr merge {pr}"));
                match gh.merge_pr(pr) {
                    Ok(()) => {
                        let view = retry_with_backoff(
                            &RetryConfig {
                                operation_name: "view_pr_after_merge",
                                ..Default::default()
                            },
                            || gh.view_pr(pr),
                        )?;
                        let sha = view.merge_commit.map(|c| c.oid).unwrap_or_default();
                        merge_output = MergingOutput {
                            merged: true,
                            merge_sha: Some(sha),
                        };
                        break;
                    }
                    Err(merge_err) => {
                        if attempt + 1 >= MAX_MERGE_REMEDIATION {
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
                                failure_reason: Some(format!(
                                    "merge failed after {} attempts: {}",
                                    MAX_MERGE_REMEDIATION, merge_err
                                )),
                            });
                        }
                    }
                }
            }
        }
    }

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

#[allow(clippy::too_many_arguments)]
pub(crate) fn worker_merge_main_and_push(
    _gh: &GhClient<'_>,
    git: &GitClient<'_>,
    learning_loop: &mut LearningLoop,
    logs: &mut Vec<crate::worker::types::WorkerLogEvent>,
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    req: &MergeRequest,
    factory: &AdapterFactory,
    registry: &PromptRegistry,
    identity: &WorkerIdentity,
    pr: u64,
    branch: &str,
    worker_id: &str,
    task_id: &str,
    attempt: u32,
    on_event: Option<&dyn Fn(WorkerStreamEvent)>,
) -> Result<(), GardenerError> {
    if git.abort_merge_if_in_progress()? {
        emit_worker_tool_command(task_id, on_event, "git merge --abort");
        append_run_log(
            "warn",
            "worker.merging.merge_from_main.stale_merge_aborted",
            serde_json::json!({
                "worker_id": worker_id,
                "pr_number": pr,
                "attempt": attempt + 1
            }),
        );
    }

    emit_worker_tool_command(task_id, on_event, "git fetch origin main");
    emit_worker_tool_command(task_id, on_event, "git merge origin/main --no-edit");
    match git.try_merge_from_main() {
        Ok(RebaseResult::Clean) => {
            append_run_log(
                "info",
                "worker.merging.merge_from_main.clean",
                serde_json::json!({
                    "worker_id": worker_id,
                    "pr_number": pr,
                    "attempt": attempt + 1
                }),
            );
            emit_worker_tool_command(task_id, on_event, &format!("git push origin {branch}"));
            git.push_with_rebase_recovery(branch)?;
            Ok(())
        }
        Ok(RebaseResult::Conflict { stderr }) => {
            let on_adapter_event = |agent_event: &crate::protocol::AgentEvent| {
                emit_adapter_tool_event(task_id, on_event, agent_event);
            };
            append_run_log(
                "warn",
                "worker.merging.merge_from_main.conflict",
                serde_json::json!({
                    "worker_id": worker_id,
                    "pr_number": pr,
                    "attempt": attempt + 1,
                    "stderr": stderr
                }),
            );
            learning_loop.ingest_failure(
                WorkerState::Merging,
                "merge from main had conflicts",
                vec![format!("stderr={stderr}")],
            );
            let conflict_tpl = merge_main_conflict_resolution_template();
            let conflict_result = run_agent_turn(AgentTurnInput {
                cfg,
                process_runner,
                scope,
                worktree_path: &req.worktree_path,
                factory,
                registry,
                learning_loop,
                identity,
                state: WorkerState::Merging,
                task_summary: &req.task_summary,
                attempt_count: req.attempt_count,
                prompt_override: Some(&conflict_tpl),
                on_event: Some(&on_adapter_event),
            })?;
            logs.push(log_event_from(&conflict_result, WorkerState::Merging));
            if conflict_result.terminal != AgentTerminal::Failure {
                emit_worker_tool_command(
                    task_id,
                    on_event,
                    "git add -A && git commit -m \"fix: merge main into branch\"",
                );
                git.commit_all("fix: merge main into branch")?;
                emit_worker_tool_command(task_id, on_event, &format!("git push origin {branch}"));
                git.push_with_rebase_recovery(branch)?;
                Ok(())
            } else {
                Err(GardenerError::Process(
                    "agent failed to resolve merge conflicts".to_string(),
                ))
            }
        }
        Err(e) => {
            append_run_log(
                "warn",
                "worker.merging.merge_from_main.failed",
                serde_json::json!({
                    "worker_id": worker_id,
                    "pr_number": pr,
                    "attempt": attempt + 1,
                    "error": e.to_string()
                }),
            );
            Err(e)
        }
    }
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

fn has_explicit_failed_checks(failed_checks: &[FailedCheck]) -> bool {
    !failed_checks.is_empty()
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
    use super::{execute_merge_phase, has_explicit_failed_checks, teardown_after_completion};
    use crate::config::AppConfig;
    use crate::fsm::MergingOutput;
    use crate::gh::FailedCheck;
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

        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN"}"#.to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergedAt":null,"mergeCommit":null,"headRefName":"gardener/manual-test","state":"OPEN"}"#.to_string(),
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

        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN"}"#.to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergedAt":null,"mergeCommit":null,"headRefName":"gardener/manual-test","state":"OPEN"}"#.to_string(),
            stderr: String::new(),
        }));
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
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: r#"{"mergedAt":"2026-03-06T12:00:00Z","mergeCommit":{"oid":"cafebabe"},"headRefName":"gardener/manual-test","state":"MERGED"}"#.to_string(),
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

    #[test]
    fn blocked_state_without_explicit_failed_checks_is_not_remediated() {
        assert!(!has_explicit_failed_checks(&[]));
    }

    #[test]
    fn blocked_state_with_explicit_failed_checks_is_remediated() {
        let failed = vec![FailedCheck {
            name: "ci".to_string(),
            link: "https://github.com/example/repo/actions/runs/1/job/2".to_string(),
            log_snippet: "failing log".to_string(),
        }];
        assert!(has_explicit_failed_checks(&failed));
    }
}
