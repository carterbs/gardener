use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::gh::GhClient;
use crate::git::{GitClient, RebaseResult};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::{merge_main_conflict_resolution_template, PromptRegistry, PromptTemplate};
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

pub const MAX_MERGE_REMEDIATION: u32 = 3;
pub const MERGEABILITY_POLL_MAX: u32 = 10;
pub const MERGEABILITY_POLL_INTERVAL: Duration = Duration::from_secs(30);

pub struct MergeLoopContext<'a> {
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
    pub gh: &'a GhClient<'a>,
    pub git: &'a GitClient<'a>,
    pub branch: &'a str,
    pub pr_number: u64,
    pub validation_command: &'a str,
    #[allow(clippy::type_complexity)]
    pub on_step: Option<&'a dyn Fn(&str, &str)>,
    pub on_agent_event: Option<&'a dyn Fn(&crate::protocol::AgentEvent)>,
}

#[derive(Debug)]
pub enum MergeLoopOutcome {
    Merged { sha: String },
    Failed { reason: String },
}

impl MergeLoopOutcome {
    pub fn is_merged(&self) -> bool {
        matches!(self, MergeLoopOutcome::Merged { .. })
    }

    pub fn merge_sha(&self) -> Option<&str> {
        match self {
            MergeLoopOutcome::Merged { sha } => Some(sha),
            MergeLoopOutcome::Failed { .. } => None,
        }
    }
}

fn step(ctx: &MergeLoopContext<'_>, label: &str, detail: &str) {
    if let Some(on_step) = ctx.on_step {
        on_step(label, detail);
    }
}

fn agent_turn_from_ctx<'a>(
    ctx: &'a MergeLoopContext<'a>,
    prompt_override: Option<&'a PromptTemplate>,
) -> AgentTurnInput<'a> {
    AgentTurnInput {
        cfg: ctx.cfg,
        process_runner: ctx.process_runner,
        scope: ctx.scope,
        worktree_path: ctx.worktree_path,
        factory: ctx.factory,
        registry: ctx.registry,
        learning_loop: ctx.learning_loop,
        identity: ctx.identity,
        state: WorkerState::Merging,
        task_summary: ctx.task_summary,
        attempt_count: ctx.attempt_count,
        prompt_override,
        on_event: ctx.on_agent_event,
    }
}

pub fn run_merge_loop(ctx: &MergeLoopContext<'_>) -> Result<MergeLoopOutcome, GardenerError> {
    let pr = ctx.pr_number;
    let branch = ctx.branch;

    step(ctx, "MERGE", &format!("starting merge loop for PR #{pr}"));

    for attempt in 0..MAX_MERGE_REMEDIATION {
        step(ctx, "POLL", &format!("polling mergeability (attempt {}/{})", attempt + 1, MAX_MERGE_REMEDIATION));
        let _ = ctx.gh.poll_mergeability(pr, MERGEABILITY_POLL_MAX, MERGEABILITY_POLL_INTERVAL)?;

        step(ctx, "MERGE", &format!("attempting merge of PR #{pr}"));
        match ctx.gh.merge_pr(pr) {
            Ok(()) => {
                let view = ctx.gh.view_pr(pr)?;
                let sha = view.merge_commit.map(|c| c.oid).unwrap_or_default();
                append_run_log("info", "merge_loop.succeeded", json!({
                    "worker_id": ctx.identity.worker_id, "pr_number": pr, "attempt": attempt + 1, "merge_sha": sha
                }));
                step(ctx, "MERGE", &format!("merge succeeded (sha={sha})"));
                return Ok(MergeLoopOutcome::Merged { sha });
            }
            Err(merge_err) => {
                step(ctx, "MERGE", &format!("merge failed: {merge_err}"));
                if attempt + 1 >= MAX_MERGE_REMEDIATION {
                    let reason = format!("merge failed after {} remediation attempts: {}", MAX_MERGE_REMEDIATION, merge_err);
                    append_run_log("error", "merge_loop.exhausted", json!({
                        "worker_id": ctx.identity.worker_id, "pr_number": pr, "attempts": MAX_MERGE_REMEDIATION, "error": merge_err.to_string()
                    }));
                    return Ok(MergeLoopOutcome::Failed { reason });
                }

                // Merge main into branch
                step(ctx, "REMEDIATE", "merging main into branch");
                match ctx.git.try_merge_from_main() {
                    Ok(RebaseResult::Clean) => {
                        append_run_log("info", "merge_loop.merge_from_main.clean", json!({
                            "worker_id": ctx.identity.worker_id, "pr_number": pr, "attempt": attempt + 1
                        }));
                        step(ctx, "REMEDIATE", "merge-from-main clean, pushing");
                        ctx.git.push_with_rebase_recovery(branch)?;
                        continue;
                    }
                    Ok(RebaseResult::Conflict { stderr }) => {
                        append_run_log("warn", "merge_loop.merge_from_main.conflict", json!({
                            "worker_id": ctx.identity.worker_id, "pr_number": pr, "attempt": attempt + 1, "stderr": stderr
                        }));
                        step(ctx, "REMEDIATE", "merge-from-main has conflicts, running agent resolution");
                        let conflict_tpl = merge_main_conflict_resolution_template();
                        let conflict_result = run_agent_turn(agent_turn_from_ctx(ctx, Some(&conflict_tpl)))?;
                        if conflict_result.terminal != AgentTerminal::Failure {
                            step(ctx, "REMEDIATE", "agent resolved conflicts, committing and pushing");
                            ctx.git.commit_all("fix: merge main into branch")?;
                            ctx.git.push_with_rebase_recovery(branch)?;
                            continue;
                        }
                        step(ctx, "REMEDIATE", "agent failed to resolve conflicts");
                    }
                    Err(e) => {
                        append_run_log("warn", "merge_loop.merge_from_main.failed", json!({
                            "worker_id": ctx.identity.worker_id, "pr_number": pr, "attempt": attempt + 1, "error": e.to_string()
                        }));
                        step(ctx, "REMEDIATE", &format!("merge-from-main failed: {e}"));
                    }
                }

                // General agent remediation
                let status = ctx.gh.check_mergeability(pr)?;
                append_run_log("warn", "merge_loop.remediation", json!({
                    "worker_id": ctx.identity.worker_id, "pr_number": pr, "attempt": attempt + 1,
                    "mergeable": format!("{:?}", status.mergeable),
                    "merge_state_status": format!("{:?}", status.merge_state_status),
                    "error": merge_err.to_string()
                }));
                step(ctx, "REMEDIATE", "running agent remediation turn");
                let remediation_result = run_agent_turn(agent_turn_from_ctx(ctx, None))?;
                if remediation_result.terminal == AgentTerminal::Failure {
                    let reason = remediation_result.payload.get("reason").and_then(|v| v.as_str()).unwrap_or("agent remediation failed").to_string();
                    step(ctx, "REMEDIATE", &format!("agent remediation failed: {reason}"));
                    return Ok(MergeLoopOutcome::Failed { reason });
                }
                step(ctx, "REMEDIATE", "agent remediation succeeded, committing and pushing");
                ctx.git.commit_all("fix: merge remediation")?;
                ctx.git.push_with_rebase_recovery(branch)?;
            }
        }
    }

    Ok(MergeLoopOutcome::Failed { reason: "merge loop completed without resolution".to_string() })
}
