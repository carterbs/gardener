use crate::agent::factory::AdapterFactory;
use crate::config::{effective_agent_for_state, effective_model_for_state, AppConfig};
use crate::errors::GardenerError;
use crate::fsm::{
    DoingOutput, FsmSnapshot, MergingOutput, ReviewVerdict, ReviewingOutput, UnderstandOutput,
    MAX_REVIEW_LOOPS,
};
use crate::gh::{generate_pr_title_body, GhClient};
use crate::git::GitClient;
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
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

const MAX_MERGE_REMEDIATION: u32 = 3;
const MERGEABILITY_POLL_MAX: u32 = 12;
const MERGEABILITY_POLL_INTERVAL: Duration = Duration::from_secs(5);

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
    append_run_log(
        "info",
        "worker.task.started",
        json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "task_summary": task_summary
        }),
    );
    let registry = PromptRegistry::v1().with_retry_rebase(attempt_count);
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

    // --- Deterministic Commit ---
    // Agent wrote code — we commit deterministically.
    let git = GitClient::new(process_runner, &worktree_path);
    git.commit_all("feat: implement task changes")?;

    // --- Deterministic Gitting ---
    fsm.transition(WorkerState::Gitting)?;
    append_run_log(
        "info",
        "worker.gitting.deterministic.started",
        json!({
            "worker_id": identity.worker_id,
            "task_id": task_id,
            "branch": branch
        }),
    );

    git.push_with_rebase_recovery(&branch)?;

    let gh = GhClient::new(process_runner, &worktree_path);
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
    let _merge_guard = merge_phase_lock()
        .lock()
        .map_err(|_| GardenerError::Process("worker merging lock poisoned".to_string()))?;
    append_run_log(
        "info",
        "worker.merging.lock.acquired",
        json!({
            "worker_id": identity.worker_id,
            "task_id": task_id,
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
        let _ = gh.poll_mergeability(pr, MERGEABILITY_POLL_MAX, MERGEABILITY_POLL_INTERVAL)?;

        match gh.merge_pr(pr) {
            Ok(()) => {
                let view = gh.view_pr(pr)?;
                let sha = view
                    .merge_commit
                    .map(|c| c.oid)
                    .unwrap_or_default();
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

                // Agent remediation turn — agent fixes code
                let remediation_result = run_agent_turn(TurnContext {
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
                logs.push(remediation_result.log_event);
                if remediation_result.terminal == AgentTerminal::Failure {
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
    let repo_root_git = GitClient::new(process_runner, &scope.working_dir);
    repo_root_git.pull_main().ok(); // best-effort sync
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
    append_run_log(
        "info",
        "worker.merging.lock.releasing",
        json!({
            "worker_id": identity.worker_id,
            "task_id": task_id
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
                &format!(
                    "state={state:?};backend={:?};attempt_count={attempt_count}",
                    effective_agent_for_state(cfg, state)
                ),
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
        execute_task, extract_failure_reason, parse_reviewing_output, parse_understand_output,
        review_artifact_path, sanitize_for_branch,
        worktree_branch_for, worktree_path_for, worktree_slug_for_task, worktree_slug_suffix,
        WORKTREE_TASK_SLUG_PREFIX_CHARS,
    };
    use crate::config::AppConfig;
    use crate::runtime::FakeProcessRunner;
    use crate::types::{RuntimeScope, WorkerState};
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
            format!(
                "gardener/worker-1-{}",
                worktree_slug_for_task("manual:tui:GARD-03")
            )
        );

        let path = worktree_path_for(
            std::path::Path::new("/repo"),
            "worker-1",
            "manual:tui:GARD-03",
        );
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
        let detail = extract_failure_reason(
            &serde_json::json!({"message":"{\"detail\":\"merge conflicted\"}"}),
        );
        assert_eq!(detail.as_deref(), Some("merge conflicted"));

        let plain = extract_failure_reason(&serde_json::json!({"reason":"hook failed"}));
        assert_eq!(plain.as_deref(), Some("hook failed"));
        assert!(extract_failure_reason(&serde_json::json!({"other":123})).is_none());
    }
}
