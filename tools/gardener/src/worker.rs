use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput, AgentTurnOutput};
use crate::config::AppConfig;
use crate::do_phase::{parse_doing_output, select_commit_message};
use crate::errors::GardenerError;
use crate::fsm::{
    DoingOutput, FsmSnapshot, MergingOutput, ReviewVerdict, ReviewingOutput, MAX_REVIEW_LOOPS,
};
use crate::gh::{generate_pr_title_body, GhClient};
use crate::git::{GitClient, RebaseResult};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::merge_loop::{MAX_MERGE_REMEDIATION, MERGEABILITY_POLL_INTERVAL, MERGEABILITY_POLL_MAX};
use crate::output_envelope::{parse_typed_payload, END_MARKER, START_MARKER};
use crate::prompt_registry::{merge_main_conflict_resolution_template, PromptRegistry};
use crate::protocol::AgentTerminal;
use crate::review_phase::parse_reviewing_output;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerActivityState, WorkerState};
use crate::understand_phase::{classify_task, parse_understand_output};
use crate::worker_identity::WorkerIdentity;
use crate::worktree::WorktreeClient;
use serde::Serialize;
use serde_json::json;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
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
    pub main_updated: bool,
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

static MERGE_PHASE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn merge_phase_lock() -> &'static Mutex<()> {
    MERGE_PHASE_LOCK.get_or_init(|| Mutex::new(()))
}

const MAX_GITTING_REMEDIATION: u32 = 3;

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

fn emit_worker_activity_state(worker_id: &str, task_id: &str, state: WorkerActivityState) {
    emit_worker_activity_state_with(worker_id, task_id, state, json!({}));
}

fn emit_worker_activity_state_with(
    worker_id: &str,
    task_id: &str,
    state: WorkerActivityState,
    details: serde_json::Value,
) {
    let mut payload = json!({
        "worker_id": worker_id,
        "task_id": task_id,
        "state": state.as_str()
    });
    if let (serde_json::Value::Object(base), serde_json::Value::Object(extra)) =
        (&mut payload, details)
    {
        for (key, value) in extra {
            base.insert(key, value);
        }
    }
    append_run_log("info", "worker.activity.state_changed", payload);
}

struct MergePhaseLockGuard<'a> {
    _guard: MutexGuard<'a, ()>,
    worker_id: String,
    task_id: String,
}

impl<'a> MergePhaseLockGuard<'a> {
    fn new(guard: MutexGuard<'a, ()>, worker_id: &str, task_id: &str) -> Self {
        Self {
            _guard: guard,
            worker_id: worker_id.to_string(),
            task_id: task_id.to_string(),
        }
    }
}

impl Drop for MergePhaseLockGuard<'_> {
    fn drop(&mut self) {
        append_run_log(
            "info",
            "worker.merging.lock.released",
            json!({
                "worker_id": self.worker_id,
                "task_id": self.task_id
            }),
        );
    }
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
    execute_task_live(
        cfg,
        process_runner,
        scope,
        worker_id,
        task_id,
        task_summary,
        attempt_count,
    )
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
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Claimed);
    append_run_log(
        "info",
        "worker.task.started",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "task_summary": task_summary
        }),
    );
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Starting);
    let registry = PromptRegistry::v1().with_retry_rebase(attempt_count);
    let identity = WorkerIdentity::new(worker_id);
    let mut fsm = FsmSnapshot::default();
    let mut learning_loop = LearningLoop::default();
    let mut logs = Vec::new();
    let factory = AdapterFactory::with_defaults();
    let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);
    let worktree_path = worktree_path_for(repo_root, task_id);
    let branch = worktree_branch_for(task_id);
    let worktree_client = WorktreeClient::new(process_runner, repo_root);
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::WorktreePreparing);
    worktree_client.create_or_resume(&worktree_path, &branch)?;
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::WorktreeReady);

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

    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Understand);
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
        on_event: None,
    })?;
    logs.push(log_event_from(&understand_result, WorkerState::Understand));
    if understand_result.terminal == AgentTerminal::Failure {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
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
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Planning);
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
            on_event: None,
        })?;
        logs.push(log_event_from(&planning_result, WorkerState::Planning));
        if planning_result.terminal == AgentTerminal::Failure {
            emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
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

    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Doing);
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
        on_event: None,
    })?;
    logs.push(log_event_from(&doing_result, WorkerState::Doing));
    if doing_result.terminal == AgentTerminal::Failure {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
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
    let doing_output = parse_doing_output(&doing_result.payload, worker_id, task_summary);
    let commit_message =
        select_commit_message(&doing_output.commit_message, worker_id, task_summary);
    fsm.on_doing_turn_completed()?;
    if fsm.state == WorkerState::Parked {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Parked);
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

    // --- Deterministic Commit ---
    // Agent wrote code — we commit deterministically.
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Commit);
    let git = GitClient::new(process_runner, &worktree_path);
    git.commit_all(&commit_message)?;

    // --- Deterministic Gitting ---
    fsm.transition(WorkerState::Gitting)?;
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Gitting);
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
                    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
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
                    return Ok(WorkerRunSummary {
                        worker_id: identity.worker_id,
                        session_id: identity.session.session_id,
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason: Some(format!(
                            "gitting failed after {} remediation attempts: {}",
                            MAX_GITTING_REMEDIATION, push_err
                        )),
                    });
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
                    on_event: None,
                })?;
                logs.push(log_event_from(&remediation_result, WorkerState::Gitting));
                if remediation_result.terminal == AgentTerminal::Failure {
                    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
                    let failure_reason = extract_failure_reason(&remediation_result.payload);
                    return Ok(WorkerRunSummary {
                        worker_id: identity.worker_id,
                        session_id: identity.session.session_id,
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason,
                    });
                }

                git.commit_all("fix: gitting remediation")?;
            }
        }
    }

    let gh = GhClient::new(process_runner, &worktree_path);
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::PrCreating);
    let (title, body) = generate_pr_title_body(process_runner, &worktree_path, task_summary)?;
    let (number, _url) = gh.create_pr(&title, &body)?;
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

    // --- Reviewing ---
    fsm.transition(WorkerState::Reviewing)?;
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Reviewing);
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
        on_event: None,
    })?;
    logs.push(log_event_from(&reviewing_result, WorkerState::Reviewing));
    if reviewing_result.terminal == AgentTerminal::Failure {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
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
            emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Parked);
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
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Doing);
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
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Merging);
    }

    // --- Deterministic Merging ---
    append_run_log(
        "info",
        "worker.merging.lock.waiting",
        json!({
            "worker_id": identity.worker_id,
            "task_id": task_id,
            "branch": branch
        }),
    );
    emit_worker_activity_state_with(
        worker_id,
        task_id,
        WorkerActivityState::MergeLockWaiting,
        json!({
            "branch": branch
        }),
    );
    let merge_guard = merge_phase_lock()
        .lock()
        .map_err(|_| GardenerError::Process("worker merging lock poisoned".to_string()))?;
    let _merge_guard = MergePhaseLockGuard::new(merge_guard, worker_id, task_id);
    append_run_log(
        "info",
        "worker.merging.lock.acquired",
        json!({
            "worker_id": identity.worker_id,
            "task_id": task_id,
            "branch": branch
        }),
    );
    emit_worker_activity_state_with(
        worker_id,
        task_id,
        WorkerActivityState::MergeLockHeld,
        json!({
            "branch": branch
        }),
    );

    let pr = pr_number;
    let mut merge_output = MergingOutput {
        merged: false,
        merge_sha: None,
    };

    for attempt in 0..MAX_MERGE_REMEDIATION {
        // Wait for GitHub to compute mergeability
        emit_worker_activity_state_with(
            worker_id,
            task_id,
            WorkerActivityState::MergePolling,
            json!({
                "pr_number": pr,
                "attempt": attempt + 1
            }),
        );
        let _ = gh.poll_mergeability(pr, MERGEABILITY_POLL_MAX, MERGEABILITY_POLL_INTERVAL)?;

        match gh.merge_pr(pr) {
            Ok(()) => {
                let view = gh.view_pr(pr)?;
                let sha = view.merge_commit.map(|c| c.oid).unwrap_or_default();
                merge_output = MergingOutput {
                    merged: true,
                    merge_sha: Some(sha),
                };
                append_run_log(
                    "info",
                    "worker.merging.deterministic.succeeded",
                    json!({
                        "worker_id": identity.worker_id,
                        "pr_number": pr,
                        "attempt": attempt + 1
                    }),
                );
                break;
            }
            Err(merge_err) => {
                if attempt + 1 >= MAX_MERGE_REMEDIATION {
                    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
                    append_run_log(
                        "error",
                        "worker.merging.deterministic.exhausted",
                        json!({
                            "worker_id": identity.worker_id,
                            "pr_number": pr,
                            "attempts": MAX_MERGE_REMEDIATION,
                            "error": merge_err.to_string()
                        }),
                    );
                    return Ok(WorkerRunSummary {
                        worker_id: identity.worker_id,
                        session_id: identity.session.session_id,
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason: Some(format!(
                            "merge failed after {} remediation attempts: {}",
                            MAX_MERGE_REMEDIATION, merge_err
                        )),
                    });
                }

                // --- Merge main into branch to get up to date ---
                emit_worker_activity_state_with(
                    worker_id,
                    task_id,
                    WorkerActivityState::MergeFromMain,
                    json!({
                        "pr_number": pr,
                        "attempt": attempt + 1
                    }),
                );
                match git.try_merge_from_main() {
                    Ok(RebaseResult::Clean) => {
                        append_run_log(
                            "info",
                            "worker.merging.merge_from_main.clean",
                            json!({
                                "worker_id": identity.worker_id,
                                "pr_number": pr,
                                "attempt": attempt + 1
                            }),
                        );
                        git.push_with_rebase_recovery(&branch)?;
                        continue;
                    }
                    Ok(RebaseResult::Conflict { stderr }) => {
                        append_run_log(
                            "warn",
                            "worker.merging.merge_from_main.conflict",
                            json!({
                                "worker_id": identity.worker_id,
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
                            worktree_path: &worktree_path,
                            factory: &factory,
                            registry: &registry,
                            learning_loop: &learning_loop,
                            identity: &identity,
                            state: WorkerState::Merging,
                            task_summary,
                            attempt_count,
                            prompt_override: Some(&conflict_tpl),
                            on_event: None,
                        })?;
                        logs.push(log_event_from(&conflict_result, WorkerState::Merging));
                        if conflict_result.terminal != AgentTerminal::Failure {
                            // Agent resolved — commit completes the merge, push, retry
                            git.commit_all("fix: merge main into branch")?;
                            git.push_with_rebase_recovery(&branch)?;
                            continue;
                        }
                        // Agent couldn't resolve — fall through to existing remediation
                    }
                    Err(e) => {
                        append_run_log(
                            "warn",
                            "worker.merging.merge_from_main.failed",
                            json!({
                                "worker_id": identity.worker_id,
                                "pr_number": pr,
                                "attempt": attempt + 1,
                                "error": e.to_string()
                            }),
                        );
                        // Fall through to existing remediation
                    }
                }

                // --- Existing merge remediation (fallback for CI/other issues) ---
                let status = gh.check_mergeability(pr)?;
                append_run_log(
                    "warn",
                    "worker.merging.deterministic.remediation",
                    json!({
                        "worker_id": identity.worker_id,
                        "pr_number": pr,
                        "attempt": attempt + 1,
                        "mergeable": format!("{:?}", status.mergeable),
                        "merge_state_status": format!("{:?}", status.merge_state_status),
                        "error": merge_err.to_string()
                    }),
                );
                emit_worker_activity_state_with(
                    worker_id,
                    task_id,
                    WorkerActivityState::MergeRemediation,
                    json!({
                        "pr_number": pr,
                        "attempt": attempt + 1
                    }),
                );
                learning_loop.ingest_failure(
                    WorkerState::Merging,
                    "merge attempt failed",
                    vec![
                        format!("pr_number={pr}"),
                        format!("attempt={}", attempt + 1),
                        format!("error={merge_err}"),
                    ],
                );

                // Agent remediation turn — agent fixes code
                let remediation_result = run_agent_turn(AgentTurnInput {
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
                    prompt_override: None,
                    on_event: None,
                })?;
                logs.push(log_event_from(&remediation_result, WorkerState::Merging));
                if remediation_result.terminal == AgentTerminal::Failure {
                    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
                    let failure_reason = extract_failure_reason(&remediation_result.payload);
                    return Ok(WorkerRunSummary {
                        worker_id: identity.worker_id,
                        session_id: identity.session.session_id,
                        final_state: WorkerState::Failed,
                        logs,
                        teardown: None,
                        failure_reason,
                    });
                }

                // We commit + push for the agent
                git.commit_all("fix: merge remediation")?;
                git.push_with_rebase_recovery(&branch)?;
            }
        }
    }

    // --- Post-merge validation ---
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::PostMergeValidation);
    let repo_root_git = GitClient::new(process_runner, &scope.working_dir);
    repo_root_git.pull_main().ok(); // best-effort sync
    if let Err(err) = repo_root_git.run_validation_command(&cfg.validation.command) {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed);
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
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Teardown);

    let teardown = teardown_after_completion(
        &worktree_client,
        &worktree_path,
        &merge_output,
        &repo_root_git,
        &identity.worker_id,
    );
    append_run_log(
        "info",
        "worker.task.complete",
        json!({
            "worker_id": identity.worker_id,
            "task_id": task_id,
            "merge_verified": teardown.merge_verified,
            "worktree_cleaned": teardown.worktree_cleaned,
            "main_updated": teardown.main_updated
        }),
    );
    emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Complete);

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

    let understand = crate::fsm::UnderstandOutput {
        task_type: classify_task(task_summary),
        reasoning: "deterministic keyword classifier".to_string(),
    };
    fsm.apply_understand(&understand)?;

    if fsm.state == WorkerState::Planning {
        fsm.transition(WorkerState::Doing)?;
    }

    let prepared = crate::agent_turn::prepare_prompt(
        cfg,
        &registry,
        &learning_loop,
        fsm.state,
        &identity.worker_id,
        task_summary,
        1,
        None,
    )?;
    logs.push(WorkerLogEvent {
        state: fsm.state,
        prompt_version: prepared.prompt_version,
        context_manifest_hash: prepared.context_manifest_hash,
    });

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

    // Deterministic gitting (simulated)
    fsm.transition(WorkerState::Gitting)?;

    // Deterministic reviewing (simulated)
    fsm.transition(WorkerState::Reviewing)?;
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

    // Deterministic merging (simulated)
    let merge_output = MergingOutput {
        merged: true,
        merge_sha: Some("deadbeef".to_string()),
    };
    learning_loop.ingest_postmerge(&merge_output, vec!["validation passed".to_string()]);

    fsm.transition(WorkerState::Complete)?;

    let teardown = TeardownReport {
        merge_verified: merge_output.merged,
        session_torn_down: true,
        sandbox_torn_down: true,
        worktree_cleaned: true,
        state_cleared: true,
        main_updated: false,
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

fn log_event_from(output: &AgentTurnOutput, state: WorkerState) -> WorkerLogEvent {
    WorkerLogEvent {
        state,
        prompt_version: output.prompt_version.clone(),
        context_manifest_hash: output.context_manifest_hash.clone(),
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

fn teardown_after_completion(
    worktree_client: &WorktreeClient<'_>,
    worktree_path: &Path,
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
        if let Err(err) = repo_git.pull_main() {
            append_run_log(
                "warn",
                "worker.teardown.pull_main_failed",
                json!({ "worker_id": worker_id, "error": err.to_string() }),
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

fn worktree_branch_for(task_id: &str) -> String {
    format!("gardener/{}", worktree_slug_for_task(task_id))
}

fn worktree_path_for(repo_root: &Path, task_id: &str) -> PathBuf {
    let base = env::var("HOME").map_or_else(
        |_| repo_root.to_path_buf(),
        |_home| PathBuf::from("/tmp/gardener-worktrees"),
    );
    base.join(worktree_slug_for_task(task_id))
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
    let base = if base.is_empty() {
        "task".to_string()
    } else {
        base
    };
    let prefix = base
        .chars()
        .take(WORKTREE_TASK_SLUG_PREFIX_CHARS)
        .collect::<String>();
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


#[cfg(test)]
mod tests {
    use super::{
        execute_task, extract_failure_reason, review_artifact_path, sanitize_for_branch,
        worktree_branch_for, worktree_path_for, worktree_slug_for_task,
        worktree_slug_suffix, WORKTREE_TASK_SLUG_PREFIX_CHARS,
    };
    use crate::config::AppConfig;
    use crate::do_phase::{fallback_commit_message, parse_doing_output, select_commit_message};
    use crate::review_phase::parse_reviewing_output;
    use crate::runtime::FakeProcessRunner;
    use crate::types::{RuntimeScope, WorkerState};
    use crate::understand_phase::parse_understand_output;
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
            crate::understand_phase::classify_task(
                "GARD-04: Build Triage mode — Live activity and Triage artifacts cards"
            ),
            crate::fsm::TaskCategory::Feature
        );
        assert_eq!(
            crate::understand_phase::classify_task(
                "GARD-02: Implement global frame — header, footer, and mode switching"
            ),
            crate::fsm::TaskCategory::Feature
        );
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
        let branch = worktree_branch_for("manual:tui:GARD-03");
        assert!(
            !branch.contains(':'),
            "branch name must not contain colon: {branch}"
        );
        assert_eq!(
            branch,
            format!("gardener/{}", worktree_slug_for_task("manual:tui:GARD-03"))
        );

        let path = worktree_path_for(std::path::Path::new("/repo"), "manual:tui:GARD-03");
        let dir_name = path
            .file_name()
            .expect("worktree path should have file name");
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
        assert!(first.len() <= WORKTREE_TASK_SLUG_PREFIX_CHARS + 1 + 16);
        let branch = worktree_branch_for("manual:tui:GARD-01");
        assert_eq!(branch.len(), "gardener/".len() + first.len());
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
    fn parse_doing_output_falls_back_when_payload_invalid() {
        let output = parse_doing_output(&serde_json::json!({"foo": "bar"}), "worker-1", "Add test");
        assert_eq!(output.summary, "Add test");
        assert!(output.files_changed.is_empty());
        assert_eq!(output.commit_message, "feat: Add test");
    }

    #[test]
    fn select_commit_message_uses_valid_message() {
        let message = select_commit_message(
            "fix(worker): wire deterministic commit to doing payload",
            "worker-1",
            "ignored",
        );
        assert_eq!(
            message,
            "fix(worker): wire deterministic commit to doing payload"
        );
    }

    #[test]
    fn select_commit_message_rejects_generic_message() {
        let message =
            select_commit_message("feat: implement task changes", "worker-1", "Enable lint");
        assert_eq!(message, "feat: Enable lint");
    }

    #[test]
    fn fallback_commit_message_handles_empty_summary() {
        let message = fallback_commit_message("   ");
        assert_eq!(message, "feat: implement requested changes");
    }

    #[test]
    fn extract_failure_reason_parses_nested_detail_field() {
        let detail = extract_failure_reason(
            &serde_json::json!({"message":"{\"detail\":\"merge conflicted\"}"}),
        );
        assert_eq!(detail.as_deref(), Some("merge conflicted"));

        let plain = extract_failure_reason(&serde_json::json!({"reason":"hook failed"}));
        assert_eq!(plain.as_deref(), Some("hook failed"));
        assert!(extract_failure_reason(&serde_json::json!({"other":123})).is_none());
    }
}
