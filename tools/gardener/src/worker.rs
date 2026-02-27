use crate::agent::factory::AdapterFactory;
use crate::config::{effective_agent_for_state, effective_model_for_state, AppConfig};
use crate::errors::GardenerError;
use crate::fsm::{
    DoingOutput, FsmSnapshot, GittingOutput, MergingOutput, ReviewVerdict, ReviewingOutput,
    UnderstandOutput, MAX_REVIEW_LOOPS,
};
use crate::git::GitClient;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::output_envelope::{parse_typed_payload, END_MARKER, START_MARKER};
use crate::prompt_context::PromptContextItem;
use crate::prompt_knowledge::to_prompt_lines;
use crate::prompt_registry::PromptRegistry;
use crate::prompts::render_state_prompt;
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use crate::worktree::WorktreeClient;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLogEvent {
    pub state: WorkerState,
    pub prompt_version: String,
    pub context_manifest_hash: String,
    pub command: Option<String>,
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
    let registry = PromptRegistry::v1().with_gitting_mode(&cfg.execution.git_output_mode);
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
            "worker.task.rebase_before_retry",
            json!({
                "worker_id": worker_id,
                "task_id": task_id,
                "attempt_count": attempt_count,
                "branch": branch
            }),
        );
        let git = GitClient::new(process_runner, &worktree_path);
        if let Err(e) = git.rebase_onto_main("main") {
            append_run_log(
                "warn",
                "worker.task.rebase_failed",
                json!({
                    "worker_id": worker_id,
                    "task_id": task_id,
                    "error": e.to_string()
                }),
            );
            return Ok(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: WorkerState::Failed,
                logs,
                teardown: None,
                failure_reason: Some(format!("rebase before retry failed: {e}")),
            });
        }
    }

    let understand_result = run_agent_turn(
        cfg,
        process_runner,
        scope,
        &worktree_path,
        &factory,
        &registry,
        &learning_loop,
        &identity,
        WorkerState::Understand,
        task_summary,
    )?;
    append_turn_log_events(&mut logs, &understand_result);
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
    let understand = parse_understand_output(&understand_result.payload, task_summary);
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
        let planning_result = run_agent_turn(
            cfg,
            process_runner,
            scope,
            &worktree_path,
            &factory,
            &registry,
            &learning_loop,
            &identity,
            WorkerState::Planning,
            task_summary,
        )?;
        append_turn_log_events(&mut logs, &planning_result);
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

    let doing_result = run_agent_turn(
        cfg,
        process_runner,
        scope,
        &worktree_path,
        &factory,
        &registry,
        &learning_loop,
        &identity,
        WorkerState::Doing,
        task_summary,
    )?;
    append_turn_log_events(&mut logs, &doing_result);
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
    let gitting_result = run_agent_turn(
        cfg,
        process_runner,
        scope,
        &worktree_path,
        &factory,
        &registry,
        &learning_loop,
        &identity,
        WorkerState::Gitting,
        task_summary,
    )?;
    append_turn_log_events(&mut logs, &gitting_result);
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
            "error",
            "worker.gitting.dirty_worktree",
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
                "gitting agent exited cleanly but left uncommitted changes in worktree".to_string(),
            ),
        });
    }

    fsm.transition(WorkerState::Reviewing)?;
    let reviewing_result = run_agent_turn(
        cfg,
        process_runner,
        scope,
        &worktree_path,
        &factory,
        &registry,
        &learning_loop,
        &identity,
        WorkerState::Reviewing,
        task_summary,
    )?;
    append_turn_log_events(&mut logs, &reviewing_result);
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

    let merging_result = run_agent_turn(
        cfg,
        process_runner,
        scope,
        &worktree_path,
        &factory,
        &registry,
        &learning_loop,
        &identity,
        WorkerState::Merging,
        task_summary,
    )?;
    append_turn_log_events(&mut logs, &merging_result);
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
    let merge_output = parse_merge_output(&merging_result.payload);
    verify_merge_output(&merge_output)?;

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

    let prepared = prepare_prompt(cfg, &registry, &learning_loop, fsm.state, task_summary)?;
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
    let prepared = prepare_prompt(cfg, &registry, &learning_loop, fsm.state, task_summary)?;
    logs.push(prepared.log_event(fsm.state));

    let gitting_output: GittingOutput = parse_typed_payload(
        &format!(
            "{START_MARKER}{{\"schema_version\":1,\"state\":\"gitting\",\"payload\":{{\"branch\":\"feat/fsm\",\"pr_number\":12,\"pr_url\":\"https://example.test/pr/12\"}}}}{END_MARKER}"
        ),
        WorkerState::Gitting,
    )?;
    verify_gitting_output(&gitting_output)?;

    fsm.transition(WorkerState::Reviewing)?;
    let prepared = prepare_prompt(cfg, &registry, &learning_loop, fsm.state, task_summary)?;
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

    let prepared = prepare_prompt(cfg, &registry, &learning_loop, fsm.state, task_summary)?;
    logs.push(prepared.log_event(fsm.state));

    let merge_output: MergingOutput = parse_typed_payload(
        &format!(
            "{START_MARKER}{{\"schema_version\":1,\"state\":\"merging\",\"payload\":{{\"merged\":true,\"merge_sha\":\"deadbeef\"}}}}{END_MARKER}"
        ),
        WorkerState::Merging,
    )?;
    verify_merge_output(&merge_output)?;
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
            command: None,
        }
    }
}

struct TurnResult {
    terminal: AgentTerminal,
    payload: serde_json::Value,
    commands: Vec<String>,
    log_event: WorkerLogEvent,
}

fn append_turn_log_events(logs: &mut Vec<WorkerLogEvent>, turn: &TurnResult) {
    let base_event = turn.log_event.clone();
    logs.push(base_event.clone());
    for command in &turn.commands {
        logs.push(WorkerLogEvent {
            command: Some(command.clone()),
            ..base_event.clone()
        });
    }
}

fn extract_action_command(payload: &Value) -> Option<String> {
    payload
        .pointer("/item/input/command")
        .or_else(|| payload.pointer("/item/command"))
        .or_else(|| payload.pointer("/command"))
        .or_else(|| payload.pointer("/input/command"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn run_agent_turn(
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    worktree_path: &Path,
    factory: &AdapterFactory,
    registry: &PromptRegistry,
    learning_loop: &LearningLoop,
    identity: &WorkerIdentity,
    state: WorkerState,
    task_summary: &str,
) -> Result<TurnResult, GardenerError> {
    let prepared = prepare_prompt(cfg, registry, learning_loop, state, task_summary)?;
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
            "output_file": output_file.display().to_string()
        }),
    );
    let max_turns = match state {
        WorkerState::Gitting => Some(50),
        _ => Some(cfg.seeding.max_turns),
    };
    let step = adapter.execute(
        process_runner,
        &crate::agent::AdapterContext {
            worker_id: identity.worker_id.clone(),
            session_id: identity.session.session_id.clone(),
            sandbox_id: identity.session.sandbox_id.clone(),
            model: model.clone(),
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
    Ok(TurnResult {
        terminal: step.terminal,
        payload: step.payload,
        commands: step
            .events
            .iter()
            .filter_map(|event| extract_action_command(&event.payload))
            .collect(),
        log_event: prepared.log_event(state),
    })
}

fn prepare_prompt(
    cfg: &AppConfig,
    registry: &PromptRegistry,
    learning_loop: &LearningLoop,
    state: WorkerState,
    task_summary: &str,
) -> Result<PreparedPrompt, GardenerError> {
    append_run_log(
        "debug",
        "worker.prompt.prepare",
        json!({
            "state": state.as_str(),
            "knowledge_entries": learning_loop.entries().len()
        }),
    );
    let token_budget = token_budget_for_state(cfg, state) as usize;
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
                        "state={state:?};backend={:?};git_output_mode={}",
                        effective_agent_for_state(cfg, state),
                        cfg.execution.git_output_mode.as_str()
                    )
                } else {
                    format!(
                        "state={state:?};backend={:?}",
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
        token_budget,
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
            "state": state.as_str(),
            "prompt_version": prompt_version,
            "context_manifest_hash": context_manifest_hash,
            "token_budget": token_budget
        }),
    );
    Ok(PreparedPrompt {
        prompt_version,
        context_manifest_hash,
        rendered: rendered.rendered,
    })
}

fn parse_understand_output(payload: &serde_json::Value, task_summary: &str) -> UnderstandOutput {
    if let Ok(parsed) = serde_json::from_value::<UnderstandOutput>(payload.clone()) {
        return parsed;
    }
    let fallback = classify_task(task_summary);
    append_run_log(
        "warn",
        "worker.understand.payload_invalid",
        json!({
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
            .unwrap_or(true),
        merge_sha: payload
            .get("merge_sha")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .or_else(|| Some("unknown".to_string())),
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
        .join(format!(
            "{}.json",
            sanitize_for_branch(task_id).to_ascii_lowercase()
        ))
}

fn verify_gitting_output(output: &GittingOutput) -> Result<(), GardenerError> {
    if output.branch.trim().is_empty() || output.pr_number == 0 || output.pr_url.trim().is_empty() {
        return Err(GardenerError::InvalidConfig(
            "gitting verification failed: missing branch/pr metadata".to_string(),
        ));
    }
    Ok(())
}

fn verify_merge_output(output: &MergingOutput) -> Result<(), GardenerError> {
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
    format!("gardener/{worker_id}-{}", sanitize_for_branch(task_id))
}

fn worktree_path_for(repo_root: &Path, worker_id: &str, task_id: &str) -> PathBuf {
    repo_root
        .join(".worktrees")
        .join(format!("{worker_id}-{}", sanitize_for_branch(task_id)))
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

fn token_budget_for_state(cfg: &AppConfig, state: WorkerState) -> u32 {
    match state {
        WorkerState::Understand => cfg.prompts.token_budget.understand,
        WorkerState::Planning => cfg.prompts.token_budget.planning,
        WorkerState::Doing => cfg.prompts.token_budget.doing,
        WorkerState::Gitting => cfg.prompts.token_budget.gitting,
        WorkerState::Reviewing => cfg.prompts.token_budget.reviewing,
        WorkerState::Merging => cfg.prompts.token_budget.merging,
        WorkerState::Seeding
        | WorkerState::Complete
        | WorkerState::Failed
        | WorkerState::Parked => cfg.prompts.token_budget.doing,
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
        verify_merge_output, worktree_branch_for, worktree_path_for,
    };
    use crate::config::AppConfig;
    use crate::fsm::{GittingOutput, MergingOutput};
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
    fn git_verification_invariants_are_enforced() {
        let err = verify_gitting_output(&GittingOutput {
            branch: String::new(),
            pr_number: 1,
            pr_url: "x".to_string(),
        })
        .expect_err("must fail");
        assert!(format!("{err}").contains("gitting verification failed"));

        let err = verify_merge_output(&MergingOutput {
            merged: true,
            merge_sha: None,
        })
        .expect_err("must fail");
        assert!(format!("{err}").contains("merge_sha required"));
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
        assert_eq!(branch, "gardener/worker-1-manual-tui-GARD-03");

        let path = worktree_path_for(
            std::path::Path::new("/repo"),
            "worker-1",
            "manual:tui:GARD-03",
        );
        let dir_name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            !dir_name.contains(':'),
            "path component must not contain colon: {dir_name}"
        );
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
            "/repo/.cache/gardener/reviews/manual-tui-gard-01.json"
        );
    }
}
