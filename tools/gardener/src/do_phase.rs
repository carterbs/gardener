use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::fsm::DoingOutput;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::PromptRegistry;
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;

pub struct DoContext<'a> {
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

pub struct DoOutcome {
    pub commit_message: String,
    pub summary: String,
    pub files_changed: Vec<String>,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}

pub fn run_do(ctx: &DoContext<'_>) -> Result<DoOutcome, GardenerError> {
    append_run_log("info", "do_phase.started", json!({ "worker_id": ctx.identity.worker_id, "task_summary": ctx.task_summary }));
    if let Some(on_step) = ctx.on_step { on_step("DO", "starting do phase"); }

    let result = run_agent_turn(AgentTurnInput {
        cfg: ctx.cfg, process_runner: ctx.process_runner, scope: ctx.scope,
        worktree_path: ctx.worktree_path, factory: ctx.factory, registry: ctx.registry,
        learning_loop: ctx.learning_loop, identity: ctx.identity,
        state: WorkerState::Doing, task_summary: ctx.task_summary,
        attempt_count: ctx.attempt_count, prompt_override: None, on_event: ctx.on_agent_event,
    })?;

    if result.terminal == AgentTerminal::Failure {
        let reason = result.payload.get("reason").and_then(|v| v.as_str()).unwrap_or("do phase failed").to_string();
        return Err(GardenerError::Process(format!("do phase agent failure: {reason}")));
    }

    let doing_output = parse_doing_output(&result.payload, &ctx.identity.worker_id, ctx.task_summary);
    let commit_message = select_commit_message(&doing_output.commit_message, &ctx.identity.worker_id, ctx.task_summary);

    if let Some(on_step) = ctx.on_step {
        on_step("DO", &format!("do phase complete: {} files changed", doing_output.files_changed.len()));
    }

    Ok(DoOutcome {
        commit_message, summary: doing_output.summary, files_changed: doing_output.files_changed,
        prompt_version: result.prompt_version, context_manifest_hash: result.context_manifest_hash,
    })
}

pub(crate) fn parse_doing_output(payload: &serde_json::Value, worker_id: &str, task_summary: &str) -> DoingOutput {
    if let Ok(parsed) = serde_json::from_value::<DoingOutput>(payload.clone()) { return parsed; }
    append_run_log("warn", "worker.doing.payload_invalid", json!({
        "worker_id": worker_id, "task_summary": task_summary, "payload": payload,
    }));
    DoingOutput { summary: task_summary.to_string(), files_changed: vec![], commit_message: fallback_commit_message(task_summary) }
}

pub(crate) fn select_commit_message(raw_message: &str, worker_id: &str, task_summary: &str) -> String {
    let trimmed = raw_message.trim();
    if is_valid_commit_message(trimmed) { return trimmed.to_string(); }
    let fallback = fallback_commit_message(task_summary);
    append_run_log("warn", "worker.doing.commit_message_invalid", json!({
        "worker_id": worker_id, "provided": raw_message, "fallback": fallback,
    }));
    fallback
}

fn is_valid_commit_message(message: &str) -> bool {
    if message.is_empty() { return false; }
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let lowered = normalized.to_ascii_lowercase();
    if matches!(lowered.as_str(),
        "feat: implement task changes" | "implement task changes" | "wip" | "update code" | "misc changes" | "fix stuff"
    ) { return false; }
    match normalized.split_once(':') {
        Some((kind, desc)) => !kind.trim().is_empty() && !desc.trim().is_empty(),
        None => false,
    }
}

pub(crate) fn fallback_commit_message(task_summary: &str) -> String {
    let first_line = task_summary.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() { return "feat: implement requested changes".to_string(); }
    let normalized = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let max_desc_len = 72usize.saturating_sub("feat: ".len());
    let desc = normalized.chars().take(max_desc_len).collect::<String>();
    if desc.is_empty() { "feat: implement requested changes".to_string() } else { format!("feat: {desc}") }
}
