use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::gh::GhClient;
use crate::git::GitClient;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::pr_creation_template;
use crate::prompt_registry::PromptRegistry;
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;

const MAX_GITTING_REMEDIATION: u32 = 3;

pub struct GitPushContext<'a> {
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
    pub branch: &'a str,
    pub commit_message: &'a str,
    #[allow(clippy::type_complexity)]
    pub on_step: Option<&'a dyn Fn(&str, &str)>,
    pub on_agent_event: Option<&'a dyn Fn(&crate::protocol::AgentEvent)>,
}

pub struct GitPushOutcome {
    pub pr_number: u64,
    pub pr_url: String,
}

pub fn run_git_push(ctx: &GitPushContext<'_>) -> Result<GitPushOutcome, GardenerError> {
    let git = GitClient::new(ctx.process_runner, ctx.worktree_path);

    if let Some(on_step) = ctx.on_step {
        on_step("GIT", "committing changes");
    }
    git.commit_all(ctx.commit_message)?;

    if let Some(on_step) = ctx.on_step {
        on_step("GIT", &format!("pushing to branch {}", ctx.branch));
    }

    for attempt in 0..MAX_GITTING_REMEDIATION {
        match git.push_with_rebase_recovery(ctx.branch) {
            Ok(()) => {
                append_run_log(
                    "info",
                    "git_phase.push.succeeded",
                    json!({
                        "worker_id": ctx.identity.worker_id, "branch": ctx.branch, "attempt": attempt + 1
                    }),
                );
                break;
            }
            Err(push_err) => {
                if attempt + 1 >= MAX_GITTING_REMEDIATION {
                    return Err(GardenerError::Process(format!(
                        "gitting failed after {MAX_GITTING_REMEDIATION} remediation attempts: {push_err}"
                    )));
                }
                append_run_log(
                    "warn",
                    "git_phase.push.remediation",
                    json!({
                        "worker_id": ctx.identity.worker_id, "branch": ctx.branch,
                        "attempt": attempt + 1, "error": push_err.to_string()
                    }),
                );
                if let Some(on_step) = ctx.on_step {
                    on_step(
                        "GIT",
                        &format!("push failed (attempt {}), running remediation", attempt + 1),
                    );
                }
                let remediation_result = run_agent_turn(AgentTurnInput {
                    cfg: ctx.cfg,
                    process_runner: ctx.process_runner,
                    scope: ctx.scope,
                    worktree_path: ctx.worktree_path,
                    factory: ctx.factory,
                    registry: ctx.registry,
                    learning_loop: ctx.learning_loop,
                    identity: ctx.identity,
                    state: WorkerState::Gitting,
                    task_summary: ctx.task_summary,
                    attempt_count: ctx.attempt_count,
                    prompt_override: None,
                    on_event: ctx.on_agent_event,
                })?;
                if remediation_result.terminal == AgentTerminal::Failure {
                    let reason = remediation_result
                        .payload
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("gitting remediation failed")
                        .to_string();
                    return Err(GardenerError::Process(format!(
                        "gitting agent remediation failed: {reason}"
                    )));
                }
                git.commit_all("fix: gitting remediation")?;
            }
        }
    }

    if let Some(on_step) = ctx.on_step {
        on_step("GIT", "creating pull request");
    }
    let gh = GhClient::new(ctx.process_runner, ctx.worktree_path);
    let pr_tpl = pr_creation_template();
    let pr_result = run_agent_turn(AgentTurnInput {
        cfg: ctx.cfg,
        process_runner: ctx.process_runner,
        scope: ctx.scope,
        worktree_path: ctx.worktree_path,
        factory: ctx.factory,
        registry: ctx.registry,
        learning_loop: ctx.learning_loop,
        identity: ctx.identity,
        state: WorkerState::Gitting,
        task_summary: ctx.task_summary,
        attempt_count: ctx.attempt_count,
        prompt_override: Some(&pr_tpl),
        on_event: ctx.on_agent_event,
    })?;
    if pr_result.terminal == AgentTerminal::Failure {
        return Err(GardenerError::Process(
            "pr creation agent failed".to_string(),
        ));
    }
    let (number, url) = gh.find_pr_for_branch(ctx.branch)?;
    append_run_log(
        "info",
        "git_phase.pr_created",
        json!({
            "worker_id": ctx.identity.worker_id, "pr_number": number, "branch": ctx.branch
        }),
    );
    if let Some(on_step) = ctx.on_step {
        on_step("GIT", &format!("PR #{number} created"));
    }

    Ok(GitPushOutcome {
        pr_number: number,
        pr_url: url,
    })
}
