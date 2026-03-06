use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::do_phase::{fallback_commit_message, parse_doing_output};
use crate::errors::GardenerError;
use crate::fsm::{DoingOutput, FsmSnapshot, ReviewVerdict, MAX_REVIEW_LOOPS};
use crate::git::GitClient;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::pr_creation_template;
use crate::protocol::AgentTerminal;
use crate::retry::{retry_with_backoff, RetryConfig};
use crate::review_phase::parse_reviewing_output;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerActivityState, WorkerState};
use crate::understand_phase::parse_understand_output;
use crate::worker::evidence::{
    collect_handoff_evidence_bundle, log_and_persist_review_output, log_event_from,
};
use crate::worker::simulated::execute_task_simulated;
use crate::worker::stream_events::{
    emit_adapter_tool_event, emit_worker_activity_state, extract_failure_reason,
};
use crate::worker::types::{
    MergeRequest, WorkerLogEvent, WorkerOutcome, WorkerRunSummary, WorkerStreamEvent,
};
use crate::worker::worktree_naming::{worktree_branch_for, worktree_path_for};
use crate::worker_identity::WorkerIdentity;
use crate::worktree::WorktreeClient;
use serde_json::json;

const MAX_GITTING_REMEDIATION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewAction {
    Rework,
    Parked,
    Handoff,
}

fn decide_review_action(verdict: ReviewVerdict, review_loops: u32) -> ReviewAction {
    if verdict == ReviewVerdict::NeedsChanges {
        if review_loops >= MAX_REVIEW_LOOPS {
            ReviewAction::Parked
        } else {
            ReviewAction::Rework
        }
    } else {
        ReviewAction::Handoff
    }
}

fn parse_merge_open_pr_number(task_summary: &str) -> Option<u64> {
    for line in task_summary.lines() {
        let trimmed = line.trim();
        let marker = "Merge open PR #";
        let Some(suffix) = trimmed.strip_prefix(marker) else {
            continue;
        };
        let digits: String = suffix.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        return digits.parse().ok();
    }
    None
}

fn should_complete_merged_pr_without_diff(
    process_runner: &dyn ProcessRunner,
    worktree_path: &std::path::Path,
    worker_id: &str,
    task_summary: &str,
) -> Result<bool, GardenerError> {
    let Some(pr_number) = parse_merge_open_pr_number(task_summary) else {
        return Ok(false);
    };
    let gh = crate::gh::GhClient::new(process_runner, worktree_path);
    let pr = match gh.view_pr(pr_number) {
        Ok(pr) => pr,
        Err(err) => {
            append_run_log(
                "warn",
                "worker.gitting.terminal_pr_check_skipped",
                json!({
                    "worker_id": worker_id,
                    "pr_number": pr_number,
                    "worktree_path": worktree_path.display().to_string(),
                    "error": err.to_string()
                }),
            );
            return Ok(false);
        }
    };
    if pr.state != "MERGED" {
        return Ok(false);
    }
    let git = GitClient::new(process_runner, worktree_path);
    let (ahead, behind) = git.head_ahead_behind_main()?;
    let should_complete = ahead == 0;
    append_run_log(
        "info",
        "worker.gitting.terminal_pr_check.completed",
        json!({
            "worker_id": worker_id,
            "pr_number": pr_number,
            "worktree_path": worktree_path.display().to_string(),
            "pr_state": pr.state,
            "ahead_of_main": ahead,
            "behind_main": behind,
            "complete_task": should_complete
        }),
    );
    Ok(should_complete)
}

fn failed_summary(
    identity: &WorkerIdentity,
    logs: Vec<WorkerLogEvent>,
    failure_reason: Option<String>,
) -> WorkerRunSummary {
    WorkerRunSummary {
        worker_id: identity.worker_id.clone(),
        session_id: identity.session.session_id.clone(),
        final_state: WorkerState::Failed,
        logs,
        teardown: None,
        failure_reason,
    }
}

fn fsm_failure_outcome(
    err: GardenerError,
    operation: &str,
    worker_id: &str,
    task_id: &str,
    identity: &WorkerIdentity,
    logs: Vec<WorkerLogEvent>,
    on_event: Option<&dyn Fn(WorkerStreamEvent)>,
) -> WorkerOutcome {
    append_run_log(
        "error",
        "worker.recovery.fsm_transition_failed",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "operation": operation,
            "error": err.to_string()
        }),
    );
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed, on_event);
    WorkerOutcome::Completed(failed_summary(
        identity,
        logs,
        Some(format!("internal FSM error: {err}")),
    ))
}

fn salvage_doing_work_from_git(
    git: &GitClient<'_>,
    pre_doing_sha: &str,
    task_summary: &str,
    worker_id: &str,
    task_id: &str,
) -> Result<Option<DoingOutput>, GardenerError> {
    let commits = git.commits_since(pre_doing_sha).unwrap_or_default();
    if let Some(subject) = commits.into_iter().next() {
        append_run_log(
            "warn",
            "worker.recovery.doing_salvage_from_commits",
            json!({
                "worker_id": worker_id,
                "task_id": task_id,
                "commit_subject": &subject
            }),
        );
        return Ok(Some(DoingOutput { summary: subject }));
    }

    if !git.worktree_is_clean()? {
        let msg = fallback_commit_message(task_summary);
        git.commit_all(&msg)?;
        append_run_log(
            "warn",
            "worker.recovery.doing_salvage_from_dirty_worktree",
            json!({
                "worker_id": worker_id,
                "task_id": task_id,
                "commit_message": &msg
            }),
        );
        return Ok(Some(DoingOutput { summary: msg }));
    }

    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_task(
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    slot_idx: usize,
    worker_id: &str,
    task_id: &str,
    task_summary: &str,
    attempt_count: i64,
    pool_started_at_ms: i64,
    task_created_at: i64,
    task_last_updated: i64,
    on_event: Option<&dyn Fn(crate::worker::types::WorkerStreamEvent)>,
) -> Result<WorkerOutcome, GardenerError> {
    let inserted_after_run_start = task_created_at >= pool_started_at_ms;
    let task_age_ms = pool_started_at_ms.saturating_sub(task_created_at);
    append_run_log(
        "debug",
        "worker.execute.dispatch",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "attempt_count": attempt_count,
            "task_summary": task_summary,
            "task_created_at": task_created_at,
            "task_last_updated": task_last_updated,
            "run_started_at_ms": pool_started_at_ms,
            "task_age_ms": task_age_ms,
            "inserted_after_run_start": inserted_after_run_start,
            "test_mode": cfg.execution.test_mode
        }),
    );
    if cfg.execution.test_mode {
        return execute_task_simulated(cfg, worker_id, task_id, task_summary)
            .map(WorkerOutcome::Completed);
    }
    execute_task_live(
        cfg,
        process_runner,
        scope,
        slot_idx,
        worker_id,
        task_id,
        task_summary,
        attempt_count,
        on_event,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_task_live(
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    slot_idx: usize,
    worker_id: &str,
    task_id: &str,
    task_summary: &str,
    attempt_count: i64,
    on_event: Option<&dyn Fn(crate::worker::types::WorkerStreamEvent)>,
) -> Result<crate::worker::types::WorkerOutcome, GardenerError> {
    let on_adapter_event = |agent_event: &crate::protocol::AgentEvent| {
        emit_adapter_tool_event(task_id, on_event, agent_event);
    };
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Claimed, on_event);
    append_run_log(
        "info",
        "worker.task.started",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "task_summary": task_summary
        }),
    );
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Starting, on_event);
    let registry = crate::prompt_registry::PromptRegistry::v1().with_retry_rebase(attempt_count);
    let identity = WorkerIdentity::new(worker_id);
    let mut fsm = FsmSnapshot::default();
    let mut learning_loop = LearningLoop::default();
    let mut logs = Vec::new();
    let factory = AdapterFactory::with_defaults();
    let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);
    let worktree_path = worktree_path_for(repo_root, task_id);
    let branch = worktree_branch_for(task_id);
    let worktree_client = WorktreeClient::new(process_runner, repo_root);
    emit_worker_activity_state(
        worker_id,
        task_id,
        WorkerActivityState::WorktreePreparing,
        on_event,
    );
    worktree_client.create_or_resume(&worktree_path, &branch)?;
    emit_worker_activity_state(
        worker_id,
        task_id,
        WorkerActivityState::WorktreeReady,
        on_event,
    );

    if attempt_count > 1 {
        append_run_log(
            "info",
            "worker.task.retry_rebase_deferred_to_agent",
            json!({
                "worker_id": worker_id,
                "task_id": task_id,
                "attempt_count": attempt_count,
                "branch": branch
            }),
        );
    }

    emit_worker_activity_state(
        worker_id,
        task_id,
        WorkerActivityState::Understand,
        on_event,
    );
    let understand_result = run_agent_turn(AgentTurnInput {
        cfg,
        process_runner,
        scope,
        worktree_path: &worktree_path,
        factory: &factory,
        registry: &registry,
        learning_loop: &learning_loop,
        identity: &identity,
        state: WorkerState::Understand,
        task_summary,
        attempt_count,
        prompt_override: None,
        on_event: Some(&on_adapter_event),
    })?;
    logs.push(log_event_from(&understand_result, WorkerState::Understand));
    if understand_result.terminal == AgentTerminal::Failure {
        append_run_log(
            "warn",
            "worker.recovery.understand_terminal_fallback",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id,
                "reason": extract_failure_reason(&understand_result.payload)
            }),
        );
    }
    let understand = parse_understand_output(&understand_result.payload, worker_id, task_summary);
    append_run_log(
        "debug",
        "worker.task.classified",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "task_type": format!("{:?}", understand.task_type),
            "reasoning": understand.reasoning,
            "worktree_path": worktree_path.display().to_string(),
            "branch": branch
        }),
    );
    if let Err(err) = fsm.apply_understand(&understand, attempt_count > 1) {
        return Ok(fsm_failure_outcome(
            err,
            "apply_understand",
            worker_id,
            task_id,
            &identity,
            logs,
            on_event,
        ));
    }

    if fsm.state == WorkerState::Planning {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Planning, on_event);
        let planning_result = run_agent_turn(AgentTurnInput {
            cfg,
            process_runner,
            scope,
            worktree_path: &worktree_path,
            factory: &factory,
            registry: &registry,
            learning_loop: &learning_loop,
            identity: &identity,
            state: WorkerState::Planning,
            task_summary,
            attempt_count,
            prompt_override: None,
            on_event: Some(&on_adapter_event),
        })?;
        logs.push(log_event_from(&planning_result, WorkerState::Planning));
        if planning_result.terminal == AgentTerminal::Failure {
            append_run_log(
                "warn",
                "worker.recovery.planning_terminal_skip",
                json!({
                    "worker_id": identity.worker_id,
                    "task_id": task_id,
                    "reason": extract_failure_reason(&planning_result.payload)
                }),
            );
        }
        if let Err(err) = fsm.transition(WorkerState::Doing) {
            return Ok(fsm_failure_outcome(
                err,
                "planning_to_doing",
                worker_id,
                task_id,
                &identity,
                logs,
                on_event,
            ));
        }
    }

    let git = GitClient::new(process_runner, &worktree_path);
    let pre_doing_sha = git.head_sha()?.unwrap_or_default();

    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Doing, on_event);
    let doing_output = match run_agent_turn(AgentTurnInput {
        cfg,
        process_runner,
        scope,
        worktree_path: &worktree_path,
        factory: &factory,
        registry: &registry,
        learning_loop: &learning_loop,
        identity: &identity,
        state: WorkerState::Doing,
        task_summary,
        attempt_count,
        prompt_override: None,
        on_event: Some(&on_adapter_event),
    }) {
        Ok(doing_result) => {
            logs.push(log_event_from(&doing_result, WorkerState::Doing));
            if doing_result.terminal == AgentTerminal::Failure {
                match salvage_doing_work_from_git(
                    &git,
                    &pre_doing_sha,
                    task_summary,
                    worker_id,
                    task_id,
                )? {
                    Some(output) => {
                        append_run_log(
                            "warn",
                            "worker.recovery.doing_terminal_failure_salvaged",
                            json!({
                                "worker_id": identity.worker_id,
                                "task_id": task_id,
                                "reason": extract_failure_reason(&doing_result.payload),
                                "summary": output.summary
                            }),
                        );
                        output
                    }
                    None => {
                        emit_worker_activity_state(
                            worker_id,
                            task_id,
                            WorkerActivityState::Failed,
                            on_event,
                        );
                        return Ok(WorkerOutcome::Completed(failed_summary(
                            &identity,
                            logs,
                            extract_failure_reason(&doing_result.payload),
                        )));
                    }
                }
            } else {
                match parse_doing_output(&doing_result.payload, worker_id, task_summary) {
                    Ok(output) => output,
                    Err(parse_err) => match salvage_doing_work_from_git(
                        &git,
                        &pre_doing_sha,
                        task_summary,
                        worker_id,
                        task_id,
                    )? {
                        Some(output) => {
                            append_run_log(
                                "warn",
                                "worker.recovery.doing_payload_salvaged",
                                json!({
                                    "worker_id": worker_id,
                                    "task_id": task_id,
                                    "parse_error": parse_err.to_string(),
                                    "summary": output.summary
                                }),
                            );
                            output
                        }
                        None => {
                            emit_worker_activity_state(
                                worker_id,
                                task_id,
                                WorkerActivityState::Failed,
                                on_event,
                            );
                            return Ok(WorkerOutcome::Completed(failed_summary(
                                &identity,
                                logs,
                                Some(parse_err.to_string()),
                            )));
                        }
                    },
                }
            }
        }
        Err(agent_err) => {
            append_run_log(
                "error",
                "worker.recovery.doing_agent_crash",
                json!({
                    "worker_id": worker_id,
                    "task_id": task_id,
                    "error": agent_err.to_string()
                }),
            );
            match salvage_doing_work_from_git(
                &git,
                &pre_doing_sha,
                task_summary,
                worker_id,
                task_id,
            )? {
                Some(output) => {
                    append_run_log(
                        "warn",
                        "worker.recovery.doing_agent_crash_salvaged",
                        json!({
                            "worker_id": worker_id,
                            "task_id": task_id,
                            "summary": output.summary
                        }),
                    );
                    output
                }
                None => {
                    emit_worker_activity_state(
                        worker_id,
                        task_id,
                        WorkerActivityState::Failed,
                        on_event,
                    );
                    return Ok(WorkerOutcome::Completed(failed_summary(
                        &identity,
                        logs,
                        Some(format!("agent process crashed: {agent_err}")),
                    )));
                }
            }
        }
    };
    let _ = doing_output;
    if let Err(err) = fsm.on_doing_turn_completed() {
        return Ok(fsm_failure_outcome(
            err,
            "doing_turn_completed",
            worker_id,
            task_id,
            &identity,
            logs,
            on_event,
        ));
    }
    if fsm.state == WorkerState::Parked {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Parked, on_event);
        append_run_log(
            "info",
            "worker.task.parked",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id
            }),
        );
        return Ok(WorkerOutcome::Completed(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Parked,
            logs,
            teardown: None,
            failure_reason: None,
        }));
    }

    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Commit, on_event);
    git.commit_all(&fallback_commit_message(task_summary))?;

    if let Err(err) = fsm.transition(WorkerState::Gitting) {
        return Ok(fsm_failure_outcome(
            err,
            "doing_to_gitting",
            worker_id,
            task_id,
            &identity,
            logs,
            on_event,
        ));
    }
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Gitting, on_event);
    append_run_log(
        "info",
        "worker.gitting.deterministic.started",
        json!({
            "worker_id": identity.worker_id,
            "task_id": task_id,
            "branch": branch
        }),
    );

    for attempt in 0..MAX_GITTING_REMEDIATION {
        match git.push_with_rebase_recovery(&branch) {
            Ok(()) => {
                append_run_log(
                    "info",
                    "worker.gitting.deterministic.succeeded",
                    json!({
                        "worker_id": identity.worker_id,
                        "task_id": task_id,
                        "branch": branch,
                        "attempt": attempt + 1
                    }),
                );
                break;
            }
            Err(push_err) => {
                if attempt + 1 >= MAX_GITTING_REMEDIATION {
                    emit_worker_activity_state(
                        worker_id,
                        task_id,
                        WorkerActivityState::Failed,
                        on_event,
                    );
                    append_run_log(
                        "error",
                        "worker.gitting.deterministic.exhausted",
                        json!({
                            "worker_id": identity.worker_id,
                            "task_id": task_id,
                            "branch": branch,
                            "attempts": MAX_GITTING_REMEDIATION,
                            "error": push_err.to_string()
                        }),
                    );
                    return Ok(WorkerOutcome::Completed(WorkerRunSummary {
                        worker_id: identity.worker_id,
                        session_id: identity.session.session_id,
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason: Some(format!(
                            "gitting failed after {} remediation attempts: {}",
                            MAX_GITTING_REMEDIATION, push_err
                        )),
                    }));
                }

                append_run_log(
                    "warn",
                    "worker.gitting.deterministic.remediation",
                    json!({
                        "worker_id": identity.worker_id,
                        "task_id": task_id,
                        "branch": branch,
                        "attempt": attempt + 1,
                        "error": push_err.to_string()
                    }),
                );
                learning_loop.ingest_failure(
                    WorkerState::Gitting,
                    "deterministic push failed",
                    vec![
                        format!("branch={branch}"),
                        format!("attempt={}", attempt + 1),
                        format!("error={push_err}"),
                    ],
                );
                emit_worker_activity_state(
                    worker_id,
                    task_id,
                    WorkerActivityState::GittingRemediation,
                    on_event,
                );
                let remediation_result = run_agent_turn(AgentTurnInput {
                    cfg,
                    process_runner,
                    scope,
                    worktree_path: &worktree_path,
                    factory: &factory,
                    registry: &registry,
                    learning_loop: &learning_loop,
                    identity: &identity,
                    state: WorkerState::Gitting,
                    task_summary,
                    attempt_count,
                    prompt_override: None,
                    on_event: Some(&on_adapter_event),
                })?;
                logs.push(log_event_from(&remediation_result, WorkerState::Gitting));
                if remediation_result.terminal == AgentTerminal::Failure {
                    emit_worker_activity_state(
                        worker_id,
                        task_id,
                        WorkerActivityState::Failed,
                        on_event,
                    );
                    let failure_reason = extract_failure_reason(&remediation_result.payload);
                    return Ok(WorkerOutcome::Completed(WorkerRunSummary {
                        worker_id: identity.worker_id,
                        session_id: identity.session.session_id,
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason,
                    }));
                }

                git.commit_all("fix: gitting remediation")?;
            }
        }
    }

    let gh = crate::gh::GhClient::new(process_runner, &worktree_path);
    emit_worker_activity_state(
        worker_id,
        task_id,
        WorkerActivityState::PrCreating,
        on_event,
    );
    let pr_tpl = pr_creation_template();
    let pr_result = run_agent_turn(AgentTurnInput {
        cfg,
        process_runner,
        scope,
        worktree_path: &worktree_path,
        factory: &factory,
        registry: &registry,
        learning_loop: &learning_loop,
        identity: &identity,
        state: WorkerState::Gitting,
        task_summary,
        attempt_count,
        prompt_override: Some(&pr_tpl),
        on_event: Some(&on_adapter_event),
    })?;
    if pr_result.terminal == AgentTerminal::Failure {
        append_run_log(
            "warn",
            "worker.recovery.pr_creation_deterministic_fallback",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id,
                "reason": extract_failure_reason(&pr_result.payload)
            }),
        );
        let title = fallback_commit_message(task_summary);
        let body = format!("Automated PR for task: {task_summary}");
        gh.create_pr(&title, &body).map(|_| ())?;
    }
    if should_complete_merged_pr_without_diff(
        process_runner,
        &worktree_path,
        &identity.worker_id,
        task_summary,
    )? {
        append_run_log(
            "info",
            "worker.gitting.terminal_pr_completion",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id,
                "branch": branch
            }),
        );
        return Ok(WorkerOutcome::Completed(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Complete,
            logs,
            teardown: None,
            failure_reason: None,
        }));
    }
    let (number, _url) = retry_with_backoff(
        &RetryConfig {
            operation_name: "find_pr_for_branch",
            ..Default::default()
        },
        || gh.find_pr_for_branch(&branch),
    )?;
    let pr_number = number;
    append_run_log(
        "info",
        "worker.gitting.deterministic.pr_created",
        json!({
            "worker_id": identity.worker_id,
            "pr_number": number,
            "branch": branch
        }),
    );

    if let Err(err) = fsm.transition(WorkerState::Reviewing) {
        return Ok(fsm_failure_outcome(
            err,
            "gitting_to_reviewing",
            worker_id,
            task_id,
            &identity,
            logs,
            on_event,
        ));
    }
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Reviewing, on_event);
    let reviewing_result = run_agent_turn(AgentTurnInput {
        cfg,
        process_runner,
        scope,
        worktree_path: &worktree_path,
        factory: &factory,
        registry: &registry,
        learning_loop: &learning_loop,
        identity: &identity,
        state: WorkerState::Reviewing,
        task_summary,
        attempt_count,
        prompt_override: None,
        on_event: Some(&on_adapter_event),
    })?;
    logs.push(log_event_from(&reviewing_result, WorkerState::Reviewing));
    if reviewing_result.terminal == AgentTerminal::Failure {
        append_run_log(
            "warn",
            "worker.recovery.review_terminal_parked",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id,
                "pr_number": pr_number,
                "reason": extract_failure_reason(&reviewing_result.payload)
            }),
        );
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Parked, on_event);
        return Ok(WorkerOutcome::Completed(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Parked,
            logs,
            teardown: None,
            failure_reason: Some("review agent failed after PR creation".to_string()),
        }));
    }
    let reviewing_output = parse_reviewing_output(&reviewing_result.payload);
    log_and_persist_review_output(scope, task_id, &identity.worker_id, &reviewing_output);
    if reviewing_output.verdict == ReviewVerdict::NeedsChanges {
        let action = decide_review_action(reviewing_output.verdict, fsm.review_loops);
        append_run_log(
            "info",
            "worker.review.needs_changes",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id,
                "review_loops": fsm.review_loops,
                "max_review_loops": MAX_REVIEW_LOOPS,
                "suggestions_count": reviewing_output.suggestions.len(),
                "suggestions": reviewing_output.suggestions
            }),
        );
        if let Err(err) = fsm.on_review_loop_back() {
            return Ok(fsm_failure_outcome(
                err,
                "review_loop_back",
                worker_id,
                task_id,
                &identity,
                logs,
                on_event,
            ));
        }
        if fsm.state != WorkerState::Parked {
            if let Err(err) = fsm.transition(WorkerState::Parked) {
                return Ok(fsm_failure_outcome(
                    err,
                    "reviewing_to_parked",
                    worker_id,
                    task_id,
                    &identity,
                    logs,
                    on_event,
                ));
            }
        }
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Parked, on_event);
        let reason = match action {
            ReviewAction::Rework => "review requested changes",
            ReviewAction::Parked => "review loop cap reached",
            ReviewAction::Handoff => "review requested changes",
        };
        append_run_log(
            "warn",
            "worker.review.rework_required",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id,
                "review_loops": fsm.review_loops,
                "reason": reason,
            }),
        );
        return Ok(WorkerOutcome::Completed(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Parked,
            logs,
            teardown: None,
            failure_reason: Some(reason.to_string()),
        }));
    } else {
        append_run_log(
            "info",
            "worker.review.approved",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id,
                "review_loops": fsm.review_loops,
                "suggestions_count": reviewing_output.suggestions.len(),
                "suggestions": reviewing_output.suggestions
            }),
        );
        if let Err(err) = fsm.transition(WorkerState::Merging) {
            return Ok(fsm_failure_outcome(
                err,
                "reviewing_to_merging",
                worker_id,
                task_id,
                &identity,
                logs,
                on_event,
            ));
        }
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Merging, on_event);
    }

    append_run_log(
        "info",
        "worker.handoff_to_merge",
        json!({
            "worker_id": identity.worker_id,
            "task_id": task_id,
            "branch": branch,
            "pr_number": pr_number
        }),
    );
    let handoff_evidence_bundle = collect_handoff_evidence_bundle(
        scope,
        task_id,
        task_summary,
        attempt_count,
        &identity.worker_id,
        &identity.session.session_id,
        &branch,
        &logs,
    );
    Ok(WorkerOutcome::HandoffToMerge(MergeRequest {
        slot_idx,
        task_id: task_id.to_string(),
        task_summary: task_summary.to_string(),
        attempt_count,
        worker_id: identity.worker_id,
        session_id: identity.session.session_id,
        worktree_path,
        branch,
        pr_number,
        logs,
        handoff_evidence_bundle,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        decide_review_action, execute_task, parse_merge_open_pr_number,
        salvage_doing_work_from_git, should_complete_merged_pr_without_diff, ReviewAction,
    };
    use crate::config::AppConfig;
    use crate::fsm::{ReviewVerdict, MAX_REVIEW_LOOPS};
    use crate::git::GitClient;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use crate::types::{RuntimeScope, WorkerState};
    use crate::worker::types::WorkerOutcome;
    use std::path::PathBuf;

    #[test]
    fn worker_executes_fsm_and_teardown_protocol() {
        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        let runner = FakeProcessRunner::default();
        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/repo"),
            repo_root: Some(PathBuf::from("/repo")),
            working_dir: PathBuf::from("/repo"),
        };

        let outcome = execute_task(
            &cfg,
            &runner,
            &scope,
            0,
            "worker-1",
            "task-1",
            "feature: add prompt packet",
            1,
            0,
            0,
            0,
            None,
        )
        .expect("ok");
        let summary = match outcome {
            WorkerOutcome::Completed(s) => s,
            WorkerOutcome::HandoffToMerge(_) => panic!("expected Completed in test mode"),
        };

        assert_eq!(summary.final_state, WorkerState::Complete);
        assert!(summary
            .logs
            .iter()
            .all(|event| !event.prompt_version.is_empty()));
        assert!(summary
            .logs
            .iter()
            .all(|event| event.context_manifest_hash.len() == 64));

        let teardown = summary.teardown.expect("teardown");
        assert!(teardown.merge_verified);
        assert!(teardown.session_torn_down);
        assert!(teardown.sandbox_torn_down);
        assert!(teardown.worktree_cleaned);
        assert!(teardown.state_cleared);
    }

    #[test]
    fn review_needs_changes_requires_rework_before_handoff() {
        let action = decide_review_action(ReviewVerdict::NeedsChanges, 0);
        assert_eq!(action, ReviewAction::Rework);
    }

    #[test]
    fn review_needs_changes_at_cap_is_parked() {
        let action = decide_review_action(ReviewVerdict::NeedsChanges, MAX_REVIEW_LOOPS);
        assert_eq!(action, ReviewAction::Parked);
    }

    #[test]
    fn parse_merge_open_pr_number_reads_manual_merge_tasks() {
        let summary = "feat: something\n\nMerge open PR #140 on branch gardener/manual-runtime";
        assert_eq!(parse_merge_open_pr_number(summary), Some(140));
        assert_eq!(parse_merge_open_pr_number("plain task"), None);
    }

    #[test]
    fn merged_pr_without_branch_diff_completes_task() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout:
                "{\"mergedAt\":\"2026-03-04T18:04:38Z\",\"mergeCommit\":{\"oid\":\"abc\"},\"headRefName\":\"gardener/manual-runtime-f5f2a381c995e9\",\"state\":\"MERGED\"}"
                    .to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "0\t0\n".to_string(),
            stderr: String::new(),
        }));

        let should_complete = should_complete_merged_pr_without_diff(
            &runner,
            std::path::Path::new("/repo"),
            "worker-1",
            "feat: lint\n\nMerge open PR #140 on branch gardener/manual-runtime-f5f2a381c995e9",
            "test-worker",
        )
        .expect("check should succeed");

        assert!(should_complete);
        let spawned = runner.spawned();
        assert_eq!(
            spawned[0].args,
            vec![
                "pr",
                "view",
                "140",
                "--json",
                "mergedAt,mergeCommit,headRefName,state"
            ]
        );
        assert_eq!(
            spawned[1].args,
            vec!["rev-list", "--left-right", "--count", "origin/main...HEAD"]
        );
    }

    #[test]
    fn merged_pr_with_commits_ahead_stays_in_pr_flow() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout:
                "{\"mergedAt\":\"2026-03-04T18:04:38Z\",\"mergeCommit\":{\"oid\":\"abc\"},\"headRefName\":\"gardener/manual-runtime-f5f2a381c995e9\",\"state\":\"MERGED\"}"
                    .to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "0\t2\n".to_string(),
            stderr: String::new(),
        }));

        let should_complete = should_complete_merged_pr_without_diff(
            &runner,
            std::path::Path::new("/repo"),
            "worker-1",
            "feat: lint\n\nMerge open PR #140 on branch gardener/manual-runtime-f5f2a381c995e9",
            "test-worker",
        )
        .expect("check should succeed");

        assert!(!should_complete);
    }

    #[test]
    fn salvage_doing_work_from_git_prefers_existing_commit_subject() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "feat: recovered work\n".to_string(),
            stderr: String::new(),
        }));

        let git = GitClient::new(&runner, "/repo");
        let salvaged = salvage_doing_work_from_git(
            &git,
            "abc123",
            "feature: add recovery",
            "worker-1",
            "task-1",
        )
        .expect("salvage should succeed");

        assert_eq!(
            salvaged.expect("should salvage").summary,
            "feat: recovered work"
        );
    }

    #[test]
    fn salvage_doing_work_from_git_commits_dirty_worktree_when_needed() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: " M src/lib.rs\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: " M src/lib.rs\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "[main abc123] feat: feature: add recovery\n".to_string(),
            stderr: String::new(),
        }));

        let git = GitClient::new(&runner, "/repo");
        let salvaged = salvage_doing_work_from_git(
            &git,
            "abc123",
            "feature: add recovery",
            "worker-1",
            "task-1",
        )
        .expect("salvage should succeed");

        assert_eq!(
            salvaged.expect("should salvage").summary,
            "feat: feature: add recovery"
        );
    }

    #[test]
    fn salvage_doing_work_from_git_returns_none_for_clean_tree_without_commits() {
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

        let git = GitClient::new(&runner, "/repo");
        let salvaged = salvage_doing_work_from_git(
            &git,
            "abc123",
            "feature: add recovery",
            "worker-1",
            "task-1",
        )
        .expect("salvage should succeed");

        assert!(salvaged.is_none());
    }
}
