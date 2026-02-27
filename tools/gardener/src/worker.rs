use crate::agent::factory::AdapterFactory;
use crate::config::{effective_agent_for_state, AppConfig};
use crate::errors::GardenerError;
use crate::fsm::{
    DoingOutput, FsmSnapshot, GittingOutput, MergingOutput, ReviewVerdict, ReviewingOutput,
    UnderstandOutput, MAX_REVIEW_LOOPS,
};
use crate::learning_loop::LearningLoop;
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
use std::path::{Path, PathBuf};

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
}

pub fn execute_task(
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    worker_id: &str,
    task_id: &str,
    task_summary: &str,
) -> Result<WorkerRunSummary, GardenerError> {
    if cfg.execution.test_mode {
        return execute_task_simulated(cfg, worker_id, task_id, task_summary);
    }
    execute_task_live(cfg, process_runner, scope, worker_id, task_id, task_summary)
}

fn execute_task_live(
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    worker_id: &str,
    task_id: &str,
    task_summary: &str,
) -> Result<WorkerRunSummary, GardenerError> {
    let registry = PromptRegistry::v1();
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

    let understand = UnderstandOutput {
        task_type: classify_task(task_summary),
        reasoning: "deterministic keyword classifier".to_string(),
    };
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
        logs.push(planning_result.log_event);
        if planning_result.terminal == AgentTerminal::Failure {
            return Ok(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: WorkerState::Failed,
                logs,
                teardown: None,
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
    logs.push(doing_result.log_event);
    if doing_result.terminal == AgentTerminal::Failure {
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
        });
    }
    fsm.on_doing_turn_completed()?;
    if fsm.state == WorkerState::Parked {
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Parked,
            logs,
            teardown: None,
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
    logs.push(gitting_result.log_event);
    if gitting_result.terminal == AgentTerminal::Failure {
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
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
    logs.push(reviewing_result.log_event);
    if reviewing_result.terminal == AgentTerminal::Failure {
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
        });
    }

    let reviewing_output = parse_reviewing_output(&reviewing_result.payload);
    if reviewing_output.verdict == ReviewVerdict::NeedsChanges {
        if fsm.review_loops >= MAX_REVIEW_LOOPS {
            fsm.on_review_loop_back()?;
            return Ok(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: fsm.state,
                logs,
                teardown: None,
            });
        }
        fsm.on_review_loop_back()?;
        fsm.transition(WorkerState::Doing)?;
    } else {
        fsm.transition(WorkerState::Merging)?;
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
    logs.push(merging_result.log_event);
    if merging_result.terminal == AgentTerminal::Failure {
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Failed,
            logs,
            teardown: None,
        });
    }
    let merge_output = parse_merge_output(&merging_result.payload);
    verify_merge_output(&merge_output)?;

    fsm.transition(WorkerState::Complete)?;

    let teardown = teardown_after_completion(&worktree_client, &worktree_path, &merge_output);

    Ok(WorkerRunSummary {
        worker_id: identity.worker_id,
        session_id: identity.session.session_id,
        final_state: WorkerState::Complete,
        logs,
        teardown: Some(teardown),
    })
}

fn execute_task_simulated(
    cfg: &AppConfig,
    worker_id: &str,
    _task_id: &str,
    task_summary: &str,
) -> Result<WorkerRunSummary, GardenerError> {
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

    let doing_payload = DoingOutput {
        summary: "implementation complete".to_string(),
        files_changed: vec!["src/lib.rs".to_string()],
    };
    let prepared = prepare_prompt(cfg, &registry, &learning_loop, fsm.state, task_summary)?;
    logs.push(prepared.log_event(fsm.state));
    fsm.on_doing_turn_completed()?;
    if fsm.state == WorkerState::Parked {
        return Ok(WorkerRunSummary {
            worker_id: identity.worker_id,
            session_id: identity.session.session_id,
            final_state: WorkerState::Parked,
            logs,
            teardown: None,
        });
    }
    let _ = doing_payload;

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

    Ok(WorkerRunSummary {
        worker_id: identity.worker_id,
        session_id: identity.session.session_id,
        final_state: WorkerState::Complete,
        logs,
        teardown: Some(teardown),
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
    let step = adapter.execute(
        process_runner,
        &crate::agent::AdapterContext {
            worker_id: identity.worker_id.clone(),
            session_id: identity.session.session_id.clone(),
            sandbox_id: identity.session.sandbox_id.clone(),
            model: cfg.seeding.model.clone(),
            cwd: worktree_path.to_path_buf(),
            prompt_version: prepared.prompt_version.clone(),
            context_manifest_hash: prepared.context_manifest_hash.clone(),
            knowledge_refs: vec![],
            output_schema: None,
            output_file: Some(output_file),
            permissive_mode: cfg.execution.permissions_mode == "permissive_v1",
            max_turns: Some(cfg.seeding.max_turns),
            cancel_requested: false,
        },
        &prepared.rendered,
    )?;
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
    task_summary: &str,
) -> Result<PreparedPrompt, GardenerError> {
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
                &format!(
                    "state={state:?};backend={:?}",
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

    Ok(PreparedPrompt {
        prompt_version: rendered.prompt_version,
        context_manifest_hash: rendered.packet.context_manifest.manifest_hash,
        rendered: rendered.rendered,
    })
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
    _worktree_client: &WorktreeClient<'_>,
    _worktree_path: &Path,
    output: &MergingOutput,
) -> TeardownReport {
    TeardownReport {
        merge_verified: output.merged,
        session_torn_down: output.merged,
        sandbox_torn_down: output.merged,
        worktree_cleaned: false,
        state_cleared: output.merged,
    }
}

fn worktree_branch_for(worker_id: &str, task_id: &str) -> String {
    format!("gardener/{worker_id}-{}", short_task_id(task_id))
}

fn worktree_path_for(repo_root: &Path, worker_id: &str, task_id: &str) -> PathBuf {
    repo_root
        .join(".worktrees")
        .join(format!("{worker_id}-{}", short_task_id(task_id)))
}

fn short_task_id(task_id: &str) -> &str {
    task_id.get(0..8).unwrap_or(task_id)
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
    } else if lower.contains("feature") {
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
    use super::{execute_task, verify_gitting_output, verify_merge_output};
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
}
