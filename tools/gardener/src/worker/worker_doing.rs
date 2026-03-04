use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::do_phase::{fallback_commit_message, parse_doing_output};
use crate::errors::GardenerError;
use crate::fsm::{DoingOutput, FsmSnapshot, MAX_REVIEW_LOOPS, ReviewVerdict};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::pr_creation_template;
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerActivityState, WorkerState};
use crate::understand_phase::parse_understand_output;
use crate::worker::evidence::{collect_handoff_evidence_bundle, log_and_persist_review_output, log_event_from};
use crate::worker::stream_events::{
    emit_adapter_tool_event, emit_worker_activity_state,
    extract_failure_reason,
};
use crate::worker::types::{MergeRequest, WorkerOutcome, WorkerRunSummary};
use crate::worker::worktree_naming::{worktree_branch_for, worktree_path_for};
use crate::worker::simulated::execute_task_simulated;
use crate::worker_identity::WorkerIdentity;
use crate::review_phase::parse_reviewing_output;
use crate::worktree::WorktreeClient;
use crate::git::GitClient;
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
        return execute_task_simulated(cfg, worker_id, task_id, task_summary).map(WorkerOutcome::Completed);
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

    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Understand, on_event);
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
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed, on_event);
        let failure_reason = extract_failure_reason(&understand_result.payload);
        append_run_log(
            "error",
            "worker.task.terminal_failure",
            json!({
                "worker_id": identity.worker_id,
                "state": "understand"
            }),
        );
        return Ok(WorkerOutcome::Completed(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason,
        }));
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
    fsm.apply_understand(&understand, attempt_count > 1)?;

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
            emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed, on_event);
            let failure_reason = extract_failure_reason(&planning_result.payload);
            append_run_log(
                "error",
                "worker.task.terminal_failure",
                json!({
                    "worker_id": identity.worker_id,
                    "state": "planning"
                }),
            );
            return Ok(WorkerOutcome::Completed(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: WorkerState::Failed,
                logs,
                teardown: None,
                failure_reason,
            }));
        }
        fsm.transition(WorkerState::Doing)?;
    }

    let git = GitClient::new(process_runner, &worktree_path);
    let pre_doing_sha = git.head_sha()?.unwrap_or_default();

    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Doing, on_event);
    let doing_result = run_agent_turn(AgentTurnInput {
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
    })?;
    logs.push(log_event_from(&doing_result, WorkerState::Doing));
    if doing_result.terminal == AgentTerminal::Failure {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed, on_event);
        let failure_reason = extract_failure_reason(&doing_result.payload);
        append_run_log(
            "error",
            "worker.task.terminal_failure",
            json!({
                "worker_id": identity.worker_id,
                "state": "doing"
            }),
        );
        return Ok(WorkerOutcome::Completed(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason,
        }));
    }
    let doing_output = match parse_doing_output(&doing_result.payload, worker_id, task_summary) {
        Ok(output) => output,
        Err(parse_err) => {
            let commits = git.commits_since(&pre_doing_sha).unwrap_or_default();
            if let Some(subject) = commits.into_iter().next() {
                append_run_log(
                    "warn",
                    "worker.doing.payload_fallback_to_git",
                    json!({
                        "worker_id": worker_id,
                        "task_id": task_id,
                        "parse_error": parse_err.to_string(),
                        "commit_subject": &subject
                    }),
                );
                DoingOutput { summary: subject }
            } else if !git.worktree_is_clean()? {
                let msg = fallback_commit_message(task_summary);
                git.commit_all(&msg)?;
                append_run_log(
                    "warn",
                    "worker.doing.payload_fallback_to_dirty_worktree",
                    json!({
                        "worker_id": worker_id,
                        "task_id": task_id,
                        "parse_error": parse_err.to_string(),
                        "commit_message": &msg
                    }),
                );
                DoingOutput { summary: msg }
            } else {
                emit_worker_activity_state(
                    worker_id,
                    task_id,
                    WorkerActivityState::Failed,
                    on_event,
                );
                return Ok(WorkerOutcome::Completed(WorkerRunSummary {
                    worker_id: identity.worker_id,
                    session_id: identity.session.session_id,
                    final_state: WorkerState::Failed,
                    logs,
                    teardown: None,
                    failure_reason: Some(parse_err.to_string()),
                }));
            }
        }
    };
    let _ = doing_output;
    fsm.on_doing_turn_completed()?;
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

    fsm.transition(WorkerState::Gitting)?;
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
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::PrCreating, on_event);
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
        return Err(GardenerError::Process("pr creation agent failed".to_string()));
    }
    let (number, _url) = gh.find_pr_for_branch(&branch)?;
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

    fsm.transition(WorkerState::Reviewing)?;
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
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed, on_event);
        let failure_reason = extract_failure_reason(&reviewing_result.payload);
        append_run_log(
            "error",
            "worker.task.terminal_failure",
            json!({
                "worker_id": identity.worker_id,
                "state": "reviewing"
            }),
        );
        return Ok(WorkerOutcome::Completed(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason,
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
        fsm.on_review_loop_back()?;
        if fsm.state != WorkerState::Parked {
            fsm.transition(WorkerState::Parked)?;
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
        fsm.transition(WorkerState::Merging)?;
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
    use super::{decide_review_action, execute_task, ReviewAction};
    use crate::config::AppConfig;
    use crate::runtime::FakeProcessRunner;
    use crate::fsm::{ReviewVerdict, MAX_REVIEW_LOOPS};
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
        assert!(summary.logs.iter().all(|event| !event.prompt_version.is_empty()));
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
}
