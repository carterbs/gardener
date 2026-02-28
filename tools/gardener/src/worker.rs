use crate::agent::factory::AdapterFactory;
use crate::config::{
    effective_agent_for_state, effective_model_for_state, AppConfig, GitOutputMode,
};
use crate::errors::GardenerError;
use crate::fsm::{
    DoingOutput, FsmSnapshot, GittingOutput, MergingOutput, ReviewVerdict, ReviewingOutput,
    UnderstandOutput, MAX_REVIEW_LOOPS,
};
use crate::git::{GitClient, RebaseResult};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::output_envelope::{parse_typed_payload, END_MARKER, START_MARKER};
use crate::prompt_context::PromptContextItem;
use crate::prompt_knowledge::to_prompt_lines;
use crate::prompt_registry::PromptRegistry;
use crate::prompts::render_state_prompt;
use crate::protocol::AgentTerminal;
use crate::replay::recorder::{emit_record, get_recording_worker_id, next_seq, timestamp_ns};
use crate::replay::recording::{AgentTurnRecord, RecordEntry};
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use crate::worktree::WorktreeClient;
use serde::Serialize;
use serde_json::json;
use std::env;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLogEvent {
    pub state: WorkerState,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeardownReport {
    pub merge_verified: bool,
    pub session_torn_down: bool,
    pub sandbox_torn_down: bool,
    pub worktree_cleaned: bool,
    pub state_cleared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRunSummary {
    pub worker_id: String,
    pub session_id: String,
    pub final_state: WorkerState,
    pub logs: Vec<WorkerLogEvent>,
    pub teardown: Option<TeardownReport>,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ReviewArtifact {
    task_id: String,
    worker_id: String,
    verdict: String,
    suggestions: Vec<String>,
    recorded_at_unix_ms: i64,
}

fn extract_failure_reason(payload: &serde_json::Value) -> Option<String> {
    let raw = payload
        .get("reason")
        .or_else(|| payload.get("message"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())?;
    // The message may be a JSON-encoded string like {"detail":"..."}
    if let Ok(inner) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(detail) = inner.get("detail").and_then(serde_json::Value::as_str) {
            return Some(detail.to_string());
        }
    }
    Some(raw.to_string())
}

pub fn execute_task(
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    worker_id: &str,
    task_id: &str,
    task_summary: &str,
    attempt_count: i64,
) -> Result<WorkerRunSummary, GardenerError> {
    append_run_log(
        "debug",
        "worker.execute.dispatch",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "attempt_count": attempt_count,
            "test_mode": cfg.execution.test_mode
        }),
    );
    if cfg.execution.test_mode {
        return execute_task_simulated(cfg, worker_id, task_id, task_summary);
    }
    execute_task_live(cfg, process_runner, scope, worker_id, task_id, task_summary, attempt_count)
}

fn execute_task_live(
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    worker_id: &str,
    task_id: &str,
    task_summary: &str,
    attempt_count: i64,
) -> Result<WorkerRunSummary, GardenerError> {
    append_run_log(
        "info",
        "worker.task.started",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "task_summary": task_summary
        }),
    );
    let registry = PromptRegistry::v1()
        .with_gitting_mode(&cfg.execution.git_output_mode)
        .with_merging_mode(&cfg.execution.git_output_mode)
        .with_retry_rebase(attempt_count);
    let identity = WorkerIdentity::new(worker_id);
    let mut fsm = FsmSnapshot::default();
    let learning_loop = LearningLoop::default();
    let mut logs = Vec::new();
    let factory = AdapterFactory::with_defaults();
    let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);
    let worktree_path = worktree_path_for(repo_root, worker_id, task_id);
    let branch = worktree_branch_for(worker_id, task_id);
    let worktree_client = WorktreeClient::new(process_runner, repo_root);
    worktree_client.create_or_resume(&worktree_path, &branch)?;

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

    let understand_result = run_agent_turn(TurnContext {
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
    })?;
    logs.push(understand_result.log_event);
    if understand_result.terminal == AgentTerminal::Failure {
        let failure_reason = extract_failure_reason(&understand_result.payload);
        append_run_log(
            "error",
            "worker.task.terminal_failure",
            json!({
                "worker_id": identity.worker_id,
                "state": "understand"
            }),
        );
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason,
        });
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
    fsm.apply_understand(&understand)?;

    if fsm.state == WorkerState::Planning {
        let planning_result = run_agent_turn(TurnContext {
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
        })?;
        logs.push(planning_result.log_event);
        if planning_result.terminal == AgentTerminal::Failure {
            let failure_reason = extract_failure_reason(&planning_result.payload);
            append_run_log(
                "error",
                "worker.task.terminal_failure",
                json!({
                    "worker_id": identity.worker_id,
                    "state": "planning"
                }),
            );
            return Ok(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: WorkerState::Failed,
                logs,
                teardown: None,
                failure_reason,
            });
        }
        fsm.transition(WorkerState::Doing)?;
    }

    let doing_result = run_agent_turn(TurnContext {
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
    })?;
    logs.push(doing_result.log_event);
    if doing_result.terminal == AgentTerminal::Failure {
        let failure_reason = extract_failure_reason(&doing_result.payload);
        append_run_log(
            "error",
            "worker.task.terminal_failure",
            json!({
                "worker_id": identity.worker_id,
                "state": "doing"
            }),
        );
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason,
        });
    }
    fsm.on_doing_turn_completed()?;
    if fsm.state == WorkerState::Parked {
        append_run_log(
            "info",
            "worker.task.parked",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id
            }),
        );
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Parked,
            logs,
            teardown: None,
            failure_reason: None,
        });
    }

    fsm.transition(WorkerState::Gitting)?;
    let gitting_result = run_agent_turn(TurnContext {
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
    })?;
    logs.push(gitting_result.log_event);
    if gitting_result.terminal == AgentTerminal::Failure {
        let failure_reason = extract_failure_reason(&gitting_result.payload);
        append_run_log(
            "error",
            "worker.task.terminal_failure",
            json!({
                "worker_id": identity.worker_id,
                "state": "gitting"
            }),
        );
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason,
        });
    }

    let git = GitClient::new(process_runner, &worktree_path);
    if !git.worktree_is_clean()? {
        append_run_log(
            "warn",
            "worker.gitting.dirty_worktree",
            json!({
                "worker_id": identity.worker_id,
                "worktree": worktree_path.display().to_string()
            }),
        );

        if cfg.execution.git_output_mode == GitOutputMode::CommitOnly {
            let recovery_registry =
                PromptRegistry::v1().with_gitting_mode(&cfg.execution.git_output_mode);
            let gitting_recovery_result = run_agent_turn(TurnContext {
                cfg,
                process_runner,
                scope,
                worktree_path: &worktree_path,
                factory: &factory,
                registry: &recovery_registry,
                learning_loop: &learning_loop,
                identity: &identity,
                state: WorkerState::Gitting,
                task_summary,
                attempt_count: attempt_count + 1,
            })?;
            logs.push(gitting_recovery_result.log_event);
            if gitting_recovery_result.terminal == AgentTerminal::Failure {
                let failure_reason = extract_failure_reason(&gitting_recovery_result.payload);
                append_run_log(
                    "error",
                    "worker.task.terminal_failure",
                    json!({
                        "worker_id": identity.worker_id,
                        "state": "gitting"
                    }),
                );
                return Ok(WorkerRunSummary {
                    worker_id: identity.worker_id,
                    session_id: identity.session.session_id,
                    final_state: WorkerState::Failed,
                    logs,
                    teardown: None,
                    failure_reason,
                });
            }

            if !git.worktree_is_clean()? {
                append_run_log(
                    "error",
                    "worker.gitting.dirty_worktree_recovery_failed",
                    json!({
                        "worker_id": identity.worker_id,
                        "worktree": worktree_path.display().to_string()
                    }),
                );
                return Ok(WorkerRunSummary {
                    worker_id: identity.worker_id,
                    session_id: identity.session.session_id,
                    final_state: WorkerState::Failed,
                    logs,
                    teardown: None,
                    failure_reason: Some(
                        "gitting agent exited cleanly but left uncommitted changes in worktree after pre-commit recovery attempt".to_string(),
                    ),
                });
            }
        } else {
            return Ok(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: WorkerState::Failed,
                logs,
                teardown: None,
                failure_reason: Some(
                    "gitting agent exited cleanly but left uncommitted changes in worktree".to_string(),
                ),
            });
        }
    }

    fsm.transition(WorkerState::Reviewing)?;
    let reviewing_result = run_agent_turn(TurnContext {
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
    })?;
    logs.push(reviewing_result.log_event);
    if reviewing_result.terminal == AgentTerminal::Failure {
        let failure_reason = extract_failure_reason(&reviewing_result.payload);
        append_run_log(
            "error",
            "worker.task.terminal_failure",
            json!({
                "worker_id": identity.worker_id,
                "state": "reviewing"
            }),
        );
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason,
        });
    }

    let reviewing_output = parse_reviewing_output(&reviewing_result.payload);
    log_and_persist_review_output(scope, task_id, &identity.worker_id, &reviewing_output);
    if reviewing_output.verdict == ReviewVerdict::NeedsChanges {
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
        if fsm.review_loops >= MAX_REVIEW_LOOPS {
            append_run_log(
                "warn",
                "worker.review.loop_cap_reached",
                json!({
                    "worker_id": identity.worker_id,
                    "task_id": task_id,
                    "review_loops": fsm.review_loops
                }),
            );
            fsm.on_review_loop_back()?;
            return Ok(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: fsm.state,
                logs,
                teardown: None,
                failure_reason: None,
            });
        }
        fsm.on_review_loop_back()?;
        fsm.transition(WorkerState::Doing)?;
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
    }

    let git = GitClient::new(process_runner, &worktree_path);
    if !git.worktree_is_clean()? {
        append_run_log(
            "error",
            "worker.merging.dirty_worktree",
            json!({
                "worker_id": identity.worker_id,
                "worktree": worktree_path.display().to_string()
            }),
        );
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason: Some("worktree has uncommitted changes; cannot merge".to_string()),
        });
    }

    // Pre-merge rebase: bring the worktree branch up to date with main before
    // creating the merge commit.
    match git.try_rebase_onto_local("main") {
        Ok(RebaseResult::Clean) => {}
        Ok(RebaseResult::Conflict { stderr }) => {
            append_run_log(
                "warn",
                "worker.merging.pre_rebase_conflict",
                json!({
                    "worker_id": identity.worker_id,
                    "task_id": task_id,
                    "stderr": stderr
                }),
            );
            let conflict_registry = registry.clone().with_conflict_resolution();
            let conflict_result = run_agent_turn(TurnContext {
                cfg,
                process_runner,
                scope,
                worktree_path: &worktree_path,
                factory: &factory,
                registry: &conflict_registry,
                learning_loop: &learning_loop,
                identity: &identity,
                state: WorkerState::Merging,
                task_summary,
                attempt_count,
            })?;
            logs.push(conflict_result.log_event);
            if conflict_result.terminal == AgentTerminal::Failure {
                let failure_reason = extract_failure_reason(&conflict_result.payload);
                append_run_log(
                    "error",
                    "worker.task.terminal_failure",
                    json!({
                        "worker_id": identity.worker_id,
                        "state": "merging_conflict_resolution"
                    }),
                );
                return Ok(WorkerRunSummary {
                    worker_id: identity.worker_id,
                    session_id: identity.session.session_id,
                    final_state: WorkerState::Failed,
                    logs,
                    teardown: None,
                    failure_reason,
                });
            }

            let conflict_output = parse_conflict_resolution_output(&conflict_result.payload);
            append_run_log(
                "info",
                "worker.merging.pre_rebase_conflict_resolution",
                json!({
                    "worker_id": identity.worker_id,
                    "task_id": task_id,
                    "resolution": conflict_output.resolution,
                    "reason": conflict_output.reason
                }),
            );
            match conflict_output.resolution.as_str() {
                "resolved" => {}
                "skipped" => {
                    let skipped_sha = conflict_output.merge_sha.unwrap_or_default();
                    let worktree_cleaned = worktree_client
                        .cleanup_on_completion(&worktree_path)
                        .map(|_| true)
                        .unwrap_or(false);
                    let merge_sha = if skipped_sha.is_empty() {
                        None
                    } else {
                        Some(skipped_sha)
                    };
                    let teardown = TeardownReport {
                        merge_verified: false,
                        session_torn_down: true,
                        sandbox_torn_down: true,
                        worktree_cleaned,
                        state_cleared: true,
                    };
                    append_run_log(
                        "info",
                        "worker.merging.skipped_on_conflict",
                        json!({
                            "worker_id": identity.worker_id,
                            "task_id": task_id,
                            "merge_sha": merge_sha
                        }),
                    );
                    return Ok(WorkerRunSummary {
                        worker_id: identity.worker_id,
                        session_id: identity.session.session_id,
                        final_state: WorkerState::Complete,
                        logs,
                        teardown: Some(teardown),
                        failure_reason: None,
                    });
                }
                "unresolvable" => {
                    if let Err(err) = git.abort_rebase() {
                        append_run_log(
                            "error",
                            "worker.merging.pre_rebase_abort_failed",
                            json!({
                                "worker_id": identity.worker_id,
                                "task_id": task_id,
                                "error": err.to_string(),
                            }),
                        );
                        return Ok(WorkerRunSummary {
                            worker_id: identity.worker_id,
                            session_id: identity.session.session_id,
                            final_state: WorkerState::Failed,
                            logs,
                            teardown: None,
                            failure_reason: Some(format!("pre-merge rebase abort failed: {err}")),
                        });
                    }
                    append_run_log(
                        "info",
                        "worker.merging.pre_rebase_unresolvable",
                        json!({
                            "worker_id": identity.worker_id,
                            "task_id": task_id,
                            "reason": conflict_output.reason
                        }),
                    );
                    return Ok(WorkerRunSummary {
                        worker_id: identity.worker_id,
                        session_id: identity.session.session_id,
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason: None,
                    });
                }
                _ => {
                    return Ok(WorkerRunSummary {
                        worker_id: identity.worker_id,
                        session_id: identity.session.session_id,
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason: Some(format!(
                            "invalid conflict resolution: {}",
                            conflict_output.resolution
                        )),
                    });
                }
            }
        }
        Err(err) => {
            append_run_log(
                "error",
                "worker.merging.pre_rebase_failed",
                json!({
                    "worker_id": identity.worker_id,
                    "task_id": task_id,
                    "error": err.to_string()
                }),
            );
            return Ok(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: WorkerState::Failed,
                logs,
                teardown: None,
                failure_reason: Some(format!("pre-merge rebase failed: {err}")),
            });
        }
    }

    let merging_result = run_agent_turn(TurnContext {
        cfg,
        process_runner,
        scope,
        worktree_path: &worktree_path,
        factory: &factory,
        registry: &registry,
        learning_loop: &learning_loop,
        identity: &identity,
        state: WorkerState::Merging,
        task_summary,
        attempt_count,
    })?;
    logs.push(merging_result.log_event);
    if merging_result.terminal == AgentTerminal::Failure {
        let failure_reason = extract_failure_reason(&merging_result.payload);
        append_run_log(
            "error",
            "worker.task.terminal_failure",
            json!({
                "worker_id": identity.worker_id,
                "state": "merging"
            }),
        );
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason,
        });
    }
    let mut merge_output = parse_merge_output(&merging_result.payload);
    if let Err(err) = verify_merge_output(identity.worker_id.as_str(), &merge_output) {
        append_run_log(
            "error",
            "worker.task.merge_verification_failed",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id,
                "error": err.to_string()
            }),
        );
        // Git-based recovery: the agent's JSON output is unreliable. Check whether
        // the branch actually landed on main before giving up.
        let repo_root_git = GitClient::new(process_runner, repo_root);
        let branch_merged = repo_root_git
            .verify_ancestor(&branch, "main")
            .unwrap_or(false);
        let recovered_sha = if branch_merged {
            repo_root_git.head_sha().unwrap_or(None)
        } else {
            None
        };
        match recovered_sha {
            Some(sha) if !sha.is_empty() => {
                append_run_log(
                    "warn",
                    "worker.merging.output.recovered_from_git",
                    json!({
                        "worker_id": identity.worker_id,
                        "task_id": task_id,
                        "sha": sha,
                    }),
                );
                merge_output = MergingOutput {
                    merged: true,
                    merge_sha: Some(sha),
                };
            }
            _ => {
                // Merge genuinely did not happen — send to unresolved, not a fatal crash.
                return Ok(WorkerRunSummary {
                    worker_id: identity.worker_id,
                    session_id: identity.session.session_id,
                    final_state: WorkerState::Failed,
                    logs,
                    teardown: None,
                    failure_reason: None,
                });
            }
        }
    }

    // Post-merge validation: run the project validation command from the repo
    // root to catch regressions introduced by conflict resolution.
    let repo_root_git = GitClient::new(process_runner, &scope.working_dir);
    if let Err(err) = repo_root_git.run_validation_command(&cfg.validation.command) {
        append_run_log(
            "error",
            "worker.merging.post_validation_failed",
            json!({
                "worker_id": identity.worker_id,
                "task_id": task_id,
                "error": err.to_string()
            }),
        );
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
            failure_reason: Some(format!("post-merge validation failed: {err}")),
        });
    }

    fsm.transition(WorkerState::Complete)?;

    let teardown = teardown_after_completion(&worktree_client, &worktree_path, &merge_output);
    append_run_log(
        "info",
        "worker.task.complete",
        json!({
            "worker_id": identity.worker_id,
            "task_id": task_id,
            "merge_verified": teardown.merge_verified,
            "worktree_cleaned": teardown.worktree_cleaned
        }),
    );

    Ok(WorkerRunSummary {
        worker_id: identity.worker_id,
        session_id: identity.session.session_id,
        final_state: WorkerState::Complete,
        logs,
        teardown: Some(teardown),
        failure_reason: None,
    })
}

fn execute_task_simulated(
    cfg: &AppConfig,
    worker_id: &str,
    _task_id: &str,
    task_summary: &str,
) -> Result<WorkerRunSummary, GardenerError> {
    append_run_log(
        "info",
        "worker.task.simulated.started",
        json!({
            "worker_id": worker_id,
            "task_summary": task_summary
        }),
    );
    let registry = PromptRegistry::v1();
    let mut identity = WorkerIdentity::new(worker_id);
    let mut fsm = FsmSnapshot::default();
    let mut learning_loop = LearningLoop::default();
    let mut logs = Vec::new();

    let understand = UnderstandOutput {
        task_type: classify_task(task_summary),
        reasoning: "deterministic keyword classifier".to_string(),
    };
    fsm.apply_understand(&understand)?;

    if fsm.state == WorkerState::Planning {
        fsm.transition(WorkerState::Doing)?;
    }

    let prepared = prepare_prompt(
        cfg,
        &registry,
        &learning_loop,
        fsm.state,
        &identity.worker_id,
        task_summary,
        1,
    )?;
    logs.push(prepared.log_event(fsm.state));

    let _doing_output: DoingOutput = parse_typed_payload(
        &format!(
            "{START_MARKER}{{\"schema_version\":1,\"state\":\"doing\",\"payload\":{{\"summary\":\"implementation complete\",\"files_changed\":[\"src/lib.rs\"],\"commit_message\":\"feat: implement task\"}}}}{END_MARKER}"
        ),
        WorkerState::Doing,
    )?;

    fsm.on_doing_turn_completed()?;
    if fsm.state == WorkerState::Parked {
        append_run_log(
            "info",
            "worker.task.simulated.parked",
            json!({
                "worker_id": identity.worker_id
            }),
        );
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Parked,
            logs,
            teardown: None,
            failure_reason: None,
        });
    }

    fsm.transition(WorkerState::Gitting)?;
    let prepared = prepare_prompt(
        cfg,
        &registry,
        &learning_loop,
        fsm.state,
        worker_id,
        task_summary,
        1,
    )?;
    logs.push(prepared.log_event(fsm.state));

    let gitting_output: GittingOutput = parse_typed_payload(
        &format!(
            "{START_MARKER}{{\"schema_version\":1,\"state\":\"gitting\",\"payload\":{{\"branch\":\"feat/fsm\",\"pr_number\":12,\"pr_url\":\"https://example.test/pr/12\"}}}}{END_MARKER}"
        ),
        WorkerState::Gitting,
    )?;
    verify_gitting_output(worker_id, &gitting_output)?;

    fsm.transition(WorkerState::Reviewing)?;
    let prepared = prepare_prompt(
        cfg,
        &registry,
        &learning_loop,
        fsm.state,
        worker_id,
        task_summary,
        1,
    )?;
    logs.push(prepared.log_event(fsm.state));

    let reviewing_output = ReviewingOutput {
        verdict: ReviewVerdict::Approve,
        suggestions: vec![],
    };
    if reviewing_output.verdict == ReviewVerdict::NeedsChanges {
        if fsm.review_loops >= MAX_REVIEW_LOOPS {
            fsm.on_review_loop_back()?;
            learning_loop.ingest_failure(
                WorkerState::Reviewing,
                "review-loop-cap-reached",
                vec!["review loop capped at 3".to_string()],
            );
            return Ok(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: fsm.state,
                logs,
                teardown: None,
                failure_reason: None,
            });
        }
        fsm.on_review_loop_back()?;
        identity.begin_retry();
        fsm.transition(WorkerState::Doing)?;
    } else {
        fsm.transition(WorkerState::Merging)?;
    }

    let prepared = prepare_prompt(
        cfg,
        &registry,
        &learning_loop,
        fsm.state,
        worker_id,
        task_summary,
        1,
    )?;
    logs.push(prepared.log_event(fsm.state));

    let merge_output: MergingOutput = parse_typed_payload(
        &format!(
            "{START_MARKER}{{\"schema_version\":1,\"state\":\"merging\",\"payload\":{{\"merged\":true,\"merge_sha\":\"deadbeef\"}}}}{END_MARKER}"
        ),
        WorkerState::Merging,
    )?;
    verify_merge_output(worker_id, &merge_output)?;
    learning_loop.ingest_postmerge(&merge_output, vec!["validation passed".to_string()]);

    fsm.transition(WorkerState::Complete)?;

    let teardown = TeardownReport {
        merge_verified: merge_output.merged,
        session_torn_down: true,
        sandbox_torn_down: true,
        worktree_cleaned: true,
        state_cleared: true,
    };

    append_run_log(
        "info",
        "worker.task.simulated.complete",
        json!({
            "worker_id": identity.worker_id,
            "merge_sha": merge_output.merge_sha
        }),
    );

    Ok(WorkerRunSummary {
        worker_id: identity.worker_id,
        session_id: identity.session.session_id,
        final_state: WorkerState::Complete,
        logs,
        teardown: Some(teardown),
        failure_reason: None,
    })
}

struct PreparedPrompt {
    prompt_version: String,
    context_manifest_hash: String,
    rendered: String,
}

impl PreparedPrompt {
    fn log_event(&self, state: WorkerState) -> WorkerLogEvent {
        WorkerLogEvent {
            state,
            prompt_version: self.prompt_version.clone(),
            context_manifest_hash: self.context_manifest_hash.clone(),
        }
    }
}

struct TurnResult {
    terminal: AgentTerminal,
    payload: serde_json::Value,
    log_event: WorkerLogEvent,
}

struct TurnContext<'a> {
    cfg: &'a AppConfig,
    process_runner: &'a dyn ProcessRunner,
    scope: &'a RuntimeScope,
    worktree_path: &'a Path,
    factory: &'a AdapterFactory,
    registry: &'a PromptRegistry,
    learning_loop: &'a LearningLoop,
    identity: &'a WorkerIdentity,
    state: WorkerState,
    task_summary: &'a str,
    attempt_count: i64,
}

fn run_agent_turn(context: TurnContext<'_>) -> Result<TurnResult, GardenerError> {
    let TurnContext {
        cfg,
        process_runner,
        scope,
        worktree_path,
        factory,
        registry,
        learning_loop,
        identity,
        state,
        task_summary,
        attempt_count,
    } = context;
    let prepared = prepare_prompt(
        cfg,
        registry,
        learning_loop,
        state,
        &identity.worker_id,
        task_summary,
        attempt_count,
    )?;
    let backend = effective_agent_for_state(cfg, state).ok_or_else(|| {
        GardenerError::InvalidConfig(format!("no backend configured for {state:?}"))
    })?;
    let model = effective_model_for_state(cfg, state);
    let adapter = factory.get(backend).ok_or_else(|| {
        GardenerError::InvalidConfig(format!("adapter not registered for {:?}", backend))
    })?;
    let output_file = scope.working_dir.join(format!(
        ".cache/gardener/worker-output-{}-{}.json",
        identity.worker_id,
        state.as_str()
    ));
    if let Some(parent) = output_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GardenerError::Io(e.to_string()))?;
    }
    let estimated_prompt_tokens = prepared.rendered.split_whitespace().count();
    append_run_log(
        "info",
        "agent.turn.started",
        json!({
            "worker_id": identity.worker_id,
            "session_id": identity.session.session_id,
            "state": state.as_str(),
            "backend": backend.as_str(),
            "model": model,
            "worktree": worktree_path.display().to_string(),
            "output_file": output_file.display().to_string(),
            "initial_prompt_est_tokens": estimated_prompt_tokens
        }),
    );
    let max_turns = Some(max_turns_for_state(cfg, state));
    let step = adapter.execute(
        process_runner,
        &crate::agent::AdapterContext {
            worker_id: identity.worker_id.clone(),
            session_id: identity.session.session_id.clone(),
            sandbox_id: identity.session.sandbox_id.clone(),
            model,
            cwd: worktree_path.to_path_buf(),
            prompt_version: prepared.prompt_version.clone(),
            context_manifest_hash: prepared.context_manifest_hash.clone(),
            output_schema: None,
            output_file: Some(output_file),
            permissive_mode: cfg.execution.permissions_mode == "permissive_v1",
            max_turns,
        },
        &prepared.rendered,
        None,
    )?;
    append_run_log(
        if step.terminal == AgentTerminal::Success {
            "info"
        } else {
            "error"
        },
        "agent.turn.finished",
        json!({
            "worker_id": identity.worker_id,
            "session_id": identity.session.session_id,
            "state": state.as_str(),
            "terminal": match step.terminal {
                AgentTerminal::Success => "success",
                AgentTerminal::Failure => "failure"
            },
            "diagnostic_count": step.diagnostics.len()
        }),
    );
    emit_record(RecordEntry::AgentTurn(AgentTurnRecord {
        seq: next_seq(),
        timestamp_ns: timestamp_ns(),
        worker_id: get_recording_worker_id(),
        state: state.as_str().to_string(),
        terminal: match step.terminal {
            AgentTerminal::Success => "success".to_string(),
            AgentTerminal::Failure => "failure".to_string(),
        },
        payload: step.payload.clone(),
        diagnostic_count: step.diagnostics.len(),
    }));
    Ok(TurnResult {
        terminal: step.terminal,
        payload: step.payload,
        log_event: prepared.log_event(state),
    })
}

fn prepare_prompt(
    cfg: &AppConfig,
    registry: &PromptRegistry,
    learning_loop: &LearningLoop,
    state: WorkerState,
    worker_id: &str,
    task_summary: &str,
    attempt_count: i64,
) -> Result<PreparedPrompt, GardenerError> {
    append_run_log(
        "debug",
        "worker.prompt.prepare",
        json!({
            "worker_id": worker_id,
            "state": state.as_str(),
            "knowledge_entries": learning_loop.entries().len()
        }),
    );
    let knowledge = to_prompt_lines(
        learning_loop.entries(),
        cfg.learning.deactivate_below_confidence,
    )
    .join("\n");

    let rendered = render_state_prompt(
        registry,
        state,
        vec![
            ctx_item(
                "task_packet",
                "task",
                "task-hash",
                "task input",
                100,
                task_summary,
            ),
            ctx_item(
                "repo_context",
                "repo",
                "repo-hash",
                "repo snapshot",
                90,
                "repo context",
            ),
            ctx_item(
                "evidence_context",
                "evidence",
                "ev-hash",
                "evidence-ranked",
                80,
                "evidence context",
            ),
            ctx_item(
                "execution_context",
                "execution",
                "exec-hash",
                "state+identity",
                70,
                &if state == WorkerState::Gitting {
                    format!(
                        "state={state:?};backend={:?};git_output_mode={};attempt_count={attempt_count}",
                        effective_agent_for_state(cfg, state),
                        cfg.execution.git_output_mode.as_str()
                    )
                } else {
                    format!(
                        "state={state:?};backend={:?};attempt_count={attempt_count}",
                        effective_agent_for_state(cfg, state)
                    )
                },
            ),
            ctx_item(
                "knowledge_context",
                "knowledge",
                "know-hash",
                "learning loop",
                60,
                if knowledge.trim().is_empty() {
                    "no prior knowledge"
                } else {
                    &knowledge
                },
            ),
        ],
    )?;

    let _parsed = parse_typed_payload::<serde_json::Value>(
        &format!(
            "{}{{\"schema_version\":1,\"state\":\"{}\",\"payload\":{{\"ok\":true}}}}{}",
            START_MARKER,
            state.as_str(),
            END_MARKER
        ),
        state,
    )?;

    let prompt_version = rendered.prompt_version;
    let context_manifest_hash = rendered.packet.context_manifest.manifest_hash;
    append_run_log(
        "debug",
        "worker.prompt.ready",
        json!({
            "worker_id": worker_id,
            "state": state.as_str(),
            "prompt_version": prompt_version,
            "context_manifest_hash": context_manifest_hash
        }),
    );
    Ok(PreparedPrompt {
        prompt_version,
        context_manifest_hash,
        rendered: rendered.rendered,
    })
}

fn parse_understand_output(
    payload: &serde_json::Value,
    worker_id: &str,
    task_summary: &str,
) -> UnderstandOutput {
    if let Ok(parsed) = serde_json::from_value::<UnderstandOutput>(payload.clone()) {
        return parsed;
    }
    let fallback = classify_task(task_summary);
    append_run_log(
        "warn",
        "worker.understand.payload_invalid",
        json!({
            "worker_id": worker_id,
            "task_summary": task_summary,
            "fallback_task_type": format!("{fallback:?}"),
            "payload": payload,
        }),
    );
    UnderstandOutput {
        task_type: fallback,
        reasoning: "fallback deterministic keyword classifier (invalid understand payload)"
            .to_string(),
    }
}

fn parse_reviewing_output(payload: &serde_json::Value) -> ReviewingOutput {
    let verdict = payload
        .get("verdict")
        .and_then(serde_json::Value::as_str)
        .map(|v| match v.to_ascii_lowercase().as_str() {
            "needs_changes" => ReviewVerdict::NeedsChanges,
            _ => ReviewVerdict::Approve,
        })
        .unwrap_or(ReviewVerdict::Approve);
    let suggestions = payload
        .get("suggestions")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ReviewingOutput {
        verdict,
        suggestions,
    }
}

fn parse_merge_output(payload: &serde_json::Value) -> MergingOutput {
    MergingOutput {
        merged: payload
            .get("merged")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        merge_sha: payload
            .get("merge_sha")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConflictResolutionOutput {
    resolution: String,
    reason: String,
    merge_sha: Option<String>,
}

fn parse_conflict_resolution_output(payload: &serde_json::Value) -> ConflictResolutionOutput {
    let raw_resolution = payload
        .get("resolution")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let resolution = match raw_resolution.as_str() {
        "resolved" => "resolved".to_string(),
        "skipped" => "skipped".to_string(),
        "unresolvable" => "unresolvable".to_string(),
        _ => "invalid".to_string(),
    };
    let reason = payload
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("missing reason")
        .trim()
        .to_string();
    let merge_sha = payload
        .get("merge_sha")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty());
    ConflictResolutionOutput {
        resolution,
        reason,
        merge_sha,
    }
}

fn log_and_persist_review_output(
    scope: &RuntimeScope,
    task_id: &str,
    worker_id: &str,
    reviewing_output: &ReviewingOutput,
) {
    let artifact = ReviewArtifact {
        task_id: task_id.to_string(),
        worker_id: worker_id.to_string(),
        verdict: match reviewing_output.verdict {
            ReviewVerdict::Approve => "approve".to_string(),
            ReviewVerdict::NeedsChanges => "needs_changes".to_string(),
        },
        suggestions: reviewing_output.suggestions.clone(),
        recorded_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    };
    let artifact_path = review_artifact_path(scope, task_id);
    if let Some(parent) = artifact_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            append_run_log(
                "warn",
                "worker.review.persist_failed",
                json!({
                    "task_id": task_id,
                    "worker_id": worker_id,
                    "path": artifact_path.display().to_string(),
                    "error": err.to_string(),
                }),
            );
            return;
        }
    }
    match serde_json::to_string_pretty(&artifact) {
        Ok(payload) => {
            if let Err(err) = std::fs::write(&artifact_path, payload) {
                append_run_log(
                    "warn",
                    "worker.review.persist_failed",
                    json!({
                        "task_id": task_id,
                        "worker_id": worker_id,
                        "path": artifact_path.display().to_string(),
                        "error": err.to_string(),
                    }),
                );
                return;
            }
            append_run_log(
                "info",
                "worker.review.persisted",
                json!({
                    "task_id": task_id,
                    "worker_id": worker_id,
                    "verdict": artifact.verdict,
                    "suggestions_count": artifact.suggestions.len(),
                    "path": artifact_path.display().to_string(),
                }),
            );
        }
        Err(err) => {
            append_run_log(
                "warn",
                "worker.review.persist_failed",
                json!({
                    "task_id": task_id,
                    "worker_id": worker_id,
                    "path": artifact_path.display().to_string(),
                    "error": err.to_string(),
                }),
            );
        }
    }
}

fn review_artifact_path(scope: &RuntimeScope, task_id: &str) -> PathBuf {
    scope
        .working_dir
        .join(".cache/gardener/reviews")
        .join(format!("{}.json", worktree_slug_for_task(task_id)))
}

fn verify_gitting_output(worker_id: &str, output: &GittingOutput) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "worker.gitting.output.verify.started",
        json!({
            "worker_id": worker_id,
            "branch": output.branch.clone(),
            "pr_number": output.pr_number,
        }),
    );
    if output.branch.trim().is_empty() || output.pr_number == 0 || output.pr_url.trim().is_empty() {
        return Err(GardenerError::InvalidConfig(
            "gitting verification failed: missing branch/pr metadata".to_string(),
        ));
    }
    append_run_log(
        "debug",
        "worker.gitting.output.verify.ok",
        json!({
            "worker_id": worker_id,
            "branch": output.branch,
            "pr_url": output.pr_url,
        }),
    );
    Ok(())
}

fn verify_merge_output(worker_id: &str, output: &MergingOutput) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "worker.merging.output.verify.started",
        json!({
            "worker_id": worker_id,
            "merged": output.merged,
        }),
    );
    if !output.merged {
        return Err(GardenerError::InvalidConfig(
            "merging verification failed: merged must be true".to_string(),
        ));
    }
    if output.merged
        && output
            .merge_sha
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        return Err(GardenerError::InvalidConfig(
            "merging verification failed: merge_sha required when merged=true".to_string(),
        ));
    }
    append_run_log(
        "debug",
        "worker.merging.output.verify.ok",
        json!({
            "worker_id": worker_id,
            "merged": output.merged,
            "merge_sha_present": output.merge_sha.is_some(),
        }),
    );
    Ok(())
}

fn teardown_after_completion(
    worktree_client: &WorktreeClient<'_>,
    worktree_path: &Path,
    output: &MergingOutput,
) -> TeardownReport {
    let worktree_cleaned = if output.merged {
        worktree_client.cleanup_on_completion(worktree_path).is_ok()
    } else {
        false
    };
    TeardownReport {
        merge_verified: output.merged,
        session_torn_down: output.merged,
        sandbox_torn_down: output.merged,
        worktree_cleaned,
        state_cleared: output.merged,
    }
}

fn worktree_branch_for(worker_id: &str, task_id: &str) -> String {
    format!("gardener/{worker_id}-{}", worktree_slug_for_task(task_id))
}

fn worktree_path_for(repo_root: &Path, worker_id: &str, task_id: &str) -> PathBuf {
    let base = env::var("HOME").map_or_else(
        |_| repo_root.to_path_buf(),
        |_home| PathBuf::from("/tmp/gardener-worktrees"),
    );
    base.join(format!("{worker_id}-{}", worktree_slug_for_task(task_id)))
}

/// Returns a git-safe slug derived from the task ID.
/// Replaces runs of non-alphanumeric characters with a single `-` and
/// truncates to 24 characters so branch names stay readable.
fn sanitize_for_branch(task_id: &str) -> String {
    let slug: String = task_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse consecutive hyphens and strip leading/trailing ones.
    let collapsed = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    collapsed.chars().take(24).collect()
}

fn worktree_slug_for_task(task_id: &str) -> String {
    let base = sanitize_for_branch(task_id);
    let base = if base.is_empty() { "task".to_string() } else { base };
    let prefix = base.chars().take(WORKTREE_TASK_SLUG_PREFIX_CHARS).collect::<String>();
    let suffix = worktree_slug_suffix(task_id);
    format!("{prefix}-{suffix}")
}

fn worktree_slug_suffix(task_id: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    for &byte in task_id.as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{:08x}", hash)
}

const WORKTREE_TASK_SLUG_PREFIX_CHARS: usize = 14;

fn ctx_item(
    section: &str,
    source_id: &str,
    source_hash: &str,
    rationale: &str,
    rank: u32,
    content: &str,
) -> PromptContextItem {
    PromptContextItem {
        section: section.to_string(),
        source_id: source_id.to_string(),
        source_hash: source_hash.to_string(),
        rationale: rationale.to_string(),
        rank,
        content: content.to_string(),
    }
}

fn max_turns_for_state(cfg: &AppConfig, state: WorkerState) -> u32 {
    match state {
        WorkerState::Understand => cfg.prompts.turn_budget.understand,
        WorkerState::Planning => cfg.prompts.turn_budget.planning,
        WorkerState::Doing => cfg.prompts.turn_budget.doing,
        WorkerState::Gitting => cfg.prompts.turn_budget.gitting,
        WorkerState::Reviewing => cfg.prompts.turn_budget.reviewing,
        WorkerState::Merging => cfg.prompts.turn_budget.merging,
        WorkerState::Seeding
        | WorkerState::Complete
        | WorkerState::Failed
        | WorkerState::Parked => cfg.prompts.turn_budget.doing,
    }
}

fn classify_task(task_summary: &str) -> crate::fsm::TaskCategory {
    let lower = task_summary.to_ascii_lowercase();
    if lower.contains("bug") || lower.contains("fix") {
        crate::fsm::TaskCategory::Bugfix
    } else if lower.contains("refactor") {
        crate::fsm::TaskCategory::Refactor
    } else if lower.contains("feature")
        || lower.contains("build")
        || lower.contains("implement")
        || lower.contains("replace")
    {
        crate::fsm::TaskCategory::Feature
    } else if lower.contains("infra") {
        crate::fsm::TaskCategory::Infra
    } else if lower.contains("chore") {
        crate::fsm::TaskCategory::Chore
    } else {
        crate::fsm::TaskCategory::Task
    }
}

#[cfg(test)]
mod tests {
    use super::{
        execute_task, review_artifact_path, sanitize_for_branch, verify_gitting_output,
        parse_merge_output, parse_conflict_resolution_output, parse_reviewing_output,
        parse_understand_output, verify_merge_output, extract_failure_reason,
        worktree_branch_for, worktree_path_for,
        worktree_slug_for_task,
        worktree_slug_suffix, WORKTREE_TASK_SLUG_PREFIX_CHARS,
    };
    use crate::config::AppConfig;
    use crate::fsm::{GittingOutput, MergingOutput};
    use crate::runtime::FakeProcessRunner;
    use crate::types::{RuntimeScope, WorkerState};
    use serde_json::json;
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
        let summary = execute_task(
            &cfg,
            &runner,
            &scope,
            "worker-1",
            "task-1",
            "feature: add prompt packet",
            1,
        )
        .expect("ok");

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
    fn classify_build_and_implement_as_feature_for_planning() {
        assert_eq!(
            super::classify_task(
                "GARD-04: Build Triage mode — Live activity and Triage artifacts cards"
            ),
            crate::fsm::TaskCategory::Feature
        );
        assert_eq!(
            super::classify_task(
                "GARD-02: Implement global frame — header, footer, and mode switching"
            ),
            crate::fsm::TaskCategory::Feature
        );
    }

    #[test]
    fn git_verification_invariants_are_enforced() {
        let err = verify_gitting_output("worker-1", &GittingOutput {
            branch: String::new(),
            pr_number: 1,
            pr_url: "x".to_string(),
        })
        .expect_err("must fail");
        assert!(format!("{err}").contains("gitting verification failed"));

        let err = verify_merge_output("worker-1", &MergingOutput {
            merged: true,
            merge_sha: None,
        })
        .expect_err("must fail");
        assert!(format!("{err}").contains("merge_sha required"));
    }

    #[test]
    fn merge_verification_requires_explicit_success_flag() {
        let err = verify_merge_output("worker-1", &MergingOutput {
            merged: false,
            merge_sha: Some("deadbeef".to_string()),
        })
        .expect_err("must fail");
        assert!(format!("{err}").contains("merged must be true"));
    }

    #[test]
    fn merge_output_default_is_not_merged() {
        let output = parse_merge_output(&json!({"merge_sha":"deadbeef"}));
        assert!(!output.merged);
        assert_eq!(output.merge_sha.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn parse_conflict_resolution_output_is_normalized() {
        let output = parse_conflict_resolution_output(
            &json!({"resolution":"Resolved","reason":"main drift","merge_sha":"abc123"}),
        );
        assert_eq!(output.resolution, "resolved");
        assert_eq!(output.reason, "main drift");
        assert_eq!(output.merge_sha.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_conflict_resolution_output_catches_invalid_resolution() {
        let output = parse_conflict_resolution_output(&json!({"resolution":"maybe","reason":"x"}));
        assert_eq!(output.resolution, "invalid");
    }

    #[test]
    fn sanitize_for_branch_strips_colons_and_other_invalid_chars() {
        // Colons in task IDs (e.g. "manual:tui:GARD-03") caused git to reject
        // the branch name with "not a valid branch name".
        assert_eq!(
            sanitize_for_branch("manual:tui:GARD-03"),
            "manual-tui-GARD-03"
        );
        assert_eq!(sanitize_for_branch("simple"), "simple");
        assert_eq!(sanitize_for_branch("abc-123"), "abc-123");
        // Spaces, dots, and slashes are also invalid in branch name components.
        assert_eq!(sanitize_for_branch("foo bar"), "foo-bar");
        assert_eq!(sanitize_for_branch("foo..bar"), "foo-bar");
        assert_eq!(sanitize_for_branch("a/b/c"), "a-b-c");
        // Consecutive invalid chars collapse to a single hyphen.
        assert_eq!(sanitize_for_branch("a::b"), "a-b");
        // Output is capped at 24 chars.
        let long = "abcdefghijklmnopqrstuvwxyz";
        assert_eq!(sanitize_for_branch(long).len(), 24);
    }

    #[test]
    fn worktree_names_are_git_safe_for_namespaced_task_ids() {
        let branch = worktree_branch_for("worker-1", "manual:tui:GARD-03");
        assert!(
            !branch.contains(':'),
            "branch name must not contain colon: {branch}"
        );
        assert_eq!(
            branch,
            format!("gardener/worker-1-{}", worktree_slug_for_task("manual:tui:GARD-03"))
        );

        let path = worktree_path_for(
            std::path::Path::new("/repo"),
            "worker-1",
            "manual:tui:GARD-03",
        );
        let dir_name = path.file_name().expect("worktree path should have file name");
        let dir_name = dir_name
            .to_str()
            .expect("worktree path should be valid UTF-8");
        assert!(
            !dir_name.contains(':'),
            "path component must not contain colon: {dir_name}"
        );
    }

    #[test]
    fn worktree_slug_for_task_is_stable_and_collision_resistant() {
        let first = worktree_slug_for_task("manual:tui:GARD-01");
        let second = worktree_slug_for_task("manual:tui:GARD-11");
        assert_ne!(first, second);
        let first_suffix = first.rsplit('-').next().unwrap_or_default();
        assert_eq!(first_suffix, worktree_slug_suffix("manual:tui:GARD-01"));
        assert_eq!(first_suffix.len(), 16);
        assert_eq!(
            second.rsplit('-').next().unwrap_or_default(),
            worktree_slug_suffix("manual:tui:GARD-11")
        );
        assert!(
            first.len() <= WORKTREE_TASK_SLUG_PREFIX_CHARS + 1 + 16
        );
        let branch = worktree_branch_for("worker-1", "manual:tui:GARD-01");
        assert_eq!(branch.len(), "gardener/worker-1-".len() + first.len());
    }

    #[test]
    fn review_artifact_path_is_task_scoped_and_git_safe() {
        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/repo"),
            repo_root: Some(PathBuf::from("/repo")),
            working_dir: PathBuf::from("/repo"),
        };
        let path = review_artifact_path(&scope, "manual:tui:GARD-01");
        assert_eq!(
            path.display().to_string(),
            format!(
                "/repo/.cache/gardener/reviews/{}.json",
                worktree_slug_for_task("manual:tui:GARD-01")
            )
        );
    }

    #[test]
    fn parse_reviewing_output_defaults_to_approve_without_verdict() {
        let output = parse_reviewing_output(&serde_json::json!({}));
        assert_eq!(output.verdict, crate::fsm::ReviewVerdict::Approve);
        assert!(output.suggestions.is_empty());
    }

    #[test]
    fn parse_reviewing_output_preserves_needs_changes_and_suggestions() {
        let output = parse_reviewing_output(&serde_json::json!({
            "verdict": "needs_changes",
            "suggestions": ["first", 2, "third"],
        }));
        assert_eq!(output.verdict, crate::fsm::ReviewVerdict::NeedsChanges);
        assert_eq!(output.suggestions, vec!["first", "third"]);
    }

    #[test]
    fn parse_understand_output_falls_back_to_classifier_when_payload_invalid() {
        let output = parse_understand_output(
            &serde_json::json!({"foo": "bar"}),
            "worker-1",
            "refactor: move prompt registry to module",
        );
        assert_eq!(output.task_type, crate::fsm::TaskCategory::Refactor);
        assert_eq!(
            output.reasoning,
            "fallback deterministic keyword classifier (invalid understand payload)"
        );
    }

    #[test]
    fn extract_failure_reason_parses_nested_detail_field() {
        let detail = extract_failure_reason(&serde_json::json!({"message":"{\"detail\":\"merge conflicted\"}"}));
        assert_eq!(detail.as_deref(), Some("merge conflicted"));

        let plain = extract_failure_reason(&serde_json::json!({"reason":"hook failed"}));
        assert_eq!(plain.as_deref(), Some("hook failed"));
        assert!(extract_failure_reason(&serde_json::json!({"other":123})).is_none());
    }
}
