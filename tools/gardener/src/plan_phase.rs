use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::PromptRegistry;
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;

pub struct PlanContext<'a> {
    pub cfg: &'a AppConfig,
    pub process_runner: &'a dyn ProcessRunner,
    pub scope: &'a RuntimeScope,
    pub worktree_path: &'a Path,
    pub factory: &'a AdapterFactory,
    pub registry: &'a PromptRegistry,
    pub learning_loop: &'a LearningLoop,
    pub identity: &'a WorkerIdentity,
    pub task_summary: &'a str,
    pub attempt_count: i64,
    #[allow(clippy::type_complexity)]
    pub on_step: Option<&'a dyn Fn(&str, &str)>,
    pub on_agent_event: Option<&'a dyn Fn(&crate::protocol::AgentEvent)>,
}

pub struct PlanOutcome {
    pub prompt_version: String,
    pub context_manifest_hash: String,
}

pub fn run_plan(ctx: &PlanContext<'_>) -> Result<PlanOutcome, GardenerError> {
    append_run_log(
        "info",
        "plan_phase.started",
        json!({ "worker_id": ctx.identity.worker_id, "task_summary": ctx.task_summary }),
    );
    if let Some(on_step) = ctx.on_step {
        on_step("PLAN", "starting planning phase");
    }

    let result = run_agent_turn(AgentTurnInput {
        cfg: ctx.cfg,
        process_runner: ctx.process_runner,
        scope: ctx.scope,
        worktree_path: ctx.worktree_path,
        factory: ctx.factory,
        registry: ctx.registry,
        learning_loop: ctx.learning_loop,
        identity: ctx.identity,
        state: WorkerState::Planning,
        task_summary: ctx.task_summary,
        attempt_count: ctx.attempt_count,
        prompt_override: None,
        on_event: ctx.on_agent_event,
    })?;

    if result.terminal == AgentTerminal::Failure {
        let reason = result
            .payload
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("planning phase failed")
            .to_string();
        return Err(GardenerError::Process(format!(
            "planning phase agent failure: {reason}"
        )));
    }

    if let Some(on_step) = ctx.on_step {
        on_step("PLAN", "planning phase complete");
    }
    Ok(PlanOutcome {
        prompt_version: result.prompt_version,
        context_manifest_hash: result.context_manifest_hash,
    })
}
