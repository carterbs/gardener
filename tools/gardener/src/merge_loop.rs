use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::gh::GhClient;
use crate::gh::{MergeStateStatus, Mergeable};
use crate::git::{GitClient, RebaseResult};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::{
    ci_failure_remediation_template, merge_main_conflict_resolution_template, PromptRegistry,
    PromptTemplate,
};
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
    pub learning_loop: &'a mut LearningLoop,
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

pub fn run_merge_loop(ctx: &mut MergeLoopContext<'_>) -> Result<MergeLoopOutcome, GardenerError> {
    let pr = ctx.pr_number;
    let branch = ctx.branch;

    step(ctx, "MERGE", &format!("starting merge loop for PR #{pr}"));

    for attempt in 0..MAX_MERGE_REMEDIATION {
        step(
            ctx,
            "POLL",
            &format!(
                "polling mergeability (attempt {}/{})",
                attempt + 1,
                MAX_MERGE_REMEDIATION
            ),
        );
        let status =
            ctx.gh
                .poll_mergeability(pr, MERGEABILITY_POLL_MAX, MERGEABILITY_POLL_INTERVAL)?;
        if let Ok(pr_view) = ctx.gh.view_pr(pr) {
            if pr_view.state.eq_ignore_ascii_case("MERGED") || pr_view.merged_at.is_some() {
                let sha = pr_view.merge_commit.map(|c| c.oid).unwrap_or_default();
                append_run_log(
                    "info",
                    "merge_loop.already_merged",
                    json!({
                        "worker_id": ctx.identity.worker_id,
                        "pr_number": pr,
                        "attempt": attempt + 1,
                        "merge_sha": sha
                    }),
                );
                step(
                    ctx,
                    "MERGE",
                    &format!("pr already merged upstream (sha={sha})"),
                );
                return Ok(MergeLoopOutcome::Merged { sha });
            }
        }

        append_run_log(
            "info",
            "merge_loop.poll_result",
            json!({
                "worker_id": ctx.identity.worker_id, "pr_number": pr, "attempt": attempt + 1,
                "mergeable": format!("{:?}", status.mergeable),
                "merge_state_status": format!("{:?}", status.merge_state_status)
            }),
        );

        // State-driven: react based on the polled state
        match (&status.mergeable, &status.merge_state_status) {
            // Clean + Mergeable → attempt merge
            (Mergeable::Mergeable, MergeStateStatus::Clean | MergeStateStatus::HasHooks) => {
                step(ctx, "MERGE", &format!("attempting merge of PR #{pr}"));
                match ctx.gh.merge_pr(pr) {
                    Ok(()) => {
                        let view = ctx.gh.view_pr(pr)?;
                        let sha = view.merge_commit.map(|c| c.oid).unwrap_or_default();
                        append_run_log(
                            "info",
                            "merge_loop.succeeded",
                            json!({
                                "worker_id": ctx.identity.worker_id, "pr_number": pr, "attempt": attempt + 1, "merge_sha": sha
                            }),
                        );
                        step(ctx, "MERGE", &format!("merge succeeded (sha={sha})"));
                        return Ok(MergeLoopOutcome::Merged { sha });
                    }
                    Err(merge_err) => {
                        step(
                            ctx,
                            "MERGE",
                            &format!("merge failed despite clean status: {merge_err}"),
                        );
                        if attempt + 1 >= MAX_MERGE_REMEDIATION {
                            return Ok(MergeLoopOutcome::Failed {
                                reason: format!(
                                    "merge failed after {} attempts: {}",
                                    MAX_MERGE_REMEDIATION, merge_err
                                ),
                            });
                        }
                    }
                }
            }
            // Behind → merge main into branch, push, re-poll
            (_, MergeStateStatus::Behind) => {
                step(ctx, "REMEDIATE", "branch is behind main, merging main");
                if let Err(e) = merge_main_and_push(ctx, branch, attempt) {
                    step(
                        ctx,
                        "REMEDIATE",
                        &format!("git merge failed ({e}), falling back to agent remediation"),
                    );
                    ctx.learning_loop.ingest_failure(
                        WorkerState::Merging,
                        "merge from main failed, agent remediation needed",
                        vec![format!("error={e}")],
                    );
                    let remediation_result = run_agent_turn(agent_turn_from_ctx(ctx, None))?;
                    if remediation_result.terminal == AgentTerminal::Failure {
                        let reason = remediation_result
                            .payload
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("agent remediation failed")
                            .to_string();
                        if attempt + 1 >= MAX_MERGE_REMEDIATION {
                            return Ok(MergeLoopOutcome::Failed { reason });
                        }
                    }
                }
                continue;
            }
            // Dirty / Conflicting → merge main to resolve, push, re-poll
            (Mergeable::Conflicting, _) | (_, MergeStateStatus::Dirty) => {
                step(
                    ctx,
                    "REMEDIATE",
                    "conflicts detected, merging main into branch",
                );
                if let Err(e) = merge_main_and_push(ctx, branch, attempt) {
                    step(
                        ctx,
                        "REMEDIATE",
                        &format!("git merge failed ({e}), falling back to agent remediation"),
                    );
                    // Git-level merge failed — let the agent fix it
                    ctx.learning_loop.ingest_failure(
                        WorkerState::Merging,
                        "merge from main failed, agent remediation needed",
                        vec![format!("error={e}")],
                    );
                    let remediation_result = run_agent_turn(agent_turn_from_ctx(ctx, None))?;
                    if remediation_result.terminal == AgentTerminal::Failure {
                        let reason = remediation_result
                            .payload
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("agent remediation failed")
                            .to_string();
                        if attempt + 1 >= MAX_MERGE_REMEDIATION {
                            return Ok(MergeLoopOutcome::Failed { reason });
                        }
                    }
                }
                continue;
            }
            // Unstable → CI failed, fetch logs and run agent
            (_, MergeStateStatus::Unstable) => {
                step(
                    ctx,
                    "REMEDIATE",
                    "CI checks failed (Unstable), fetching failure logs",
                );
                let failed_checks = ctx.gh.fetch_failed_checks(pr).unwrap_or_default();
                let evidence: Vec<String> = failed_checks
                    .iter()
                    .map(|c| {
                        format!(
                            "## CI check: {}\nLink: {}\n\n```\n{}\n```",
                            c.name, c.link, c.log_snippet
                        )
                    })
                    .collect();
                ctx.learning_loop.ingest_failure(
                    WorkerState::Merging,
                    "CI checks failed",
                    evidence,
                );
                step(ctx, "REMEDIATE", "running agent CI failure remediation");
                let ci_tpl = ci_failure_remediation_template();
                let ci_result = run_agent_turn(agent_turn_from_ctx(ctx, Some(&ci_tpl)))?;
                if ci_result.terminal == AgentTerminal::Failure {
                    let reason = ci_result
                        .payload
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("agent CI remediation failed")
                        .to_string();
                    step(
                        ctx,
                        "REMEDIATE",
                        &format!("agent CI remediation failed: {reason}"),
                    );
                    if attempt + 1 >= MAX_MERGE_REMEDIATION {
                        return Ok(MergeLoopOutcome::Failed { reason });
                    }
                }
                continue;
            }
            // Blocked → required checks failed; fetch logs and remediate if actionable
            (_, MergeStateStatus::Blocked) => {
                step(
                    ctx,
                    "REMEDIATE",
                    "PR is blocked, checking for failed required checks",
                );
                let failed_checks = ctx.gh.fetch_failed_checks(pr).unwrap_or_default();
                if failed_checks.is_empty() {
                    // No failed checks — blocked by policy (reviews, etc.), not actionable
                    step(
                        ctx,
                        "REMEDIATE",
                        "no failed checks found; blocked by branch protection policy",
                    );
                    return Ok(MergeLoopOutcome::Failed {
                        reason:
                            "PR is blocked by branch protection rules (no failed CI checks found)"
                                .to_string(),
                    });
                }
                let evidence: Vec<String> = failed_checks
                    .iter()
                    .map(|c| {
                        format!(
                            "## CI check: {}\nLink: {}\n\n```\n{}\n```",
                            c.name, c.link, c.log_snippet
                        )
                    })
                    .collect();
                ctx.learning_loop.ingest_failure(
                    WorkerState::Merging,
                    "required CI checks failed (Blocked)",
                    evidence,
                );
                step(ctx, "REMEDIATE", "running agent CI failure remediation");
                let ci_tpl = ci_failure_remediation_template();
                let ci_result = run_agent_turn(agent_turn_from_ctx(ctx, Some(&ci_tpl)))?;
                if ci_result.terminal == AgentTerminal::Failure {
                    let reason = ci_result
                        .payload
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("agent CI remediation failed")
                        .to_string();
                    step(
                        ctx,
                        "REMEDIATE",
                        &format!("agent CI remediation failed: {reason}"),
                    );
                    if attempt + 1 >= MAX_MERGE_REMEDIATION {
                        return Ok(MergeLoopOutcome::Failed { reason });
                    }
                }
                continue;
            }
            // Fallback: unexpected state → try merge anyway
            _ => {
                step(
                    ctx,
                    "MERGE",
                    &format!(
                        "fallback: state {:?}/{:?}, attempting merge",
                        status.mergeable, status.merge_state_status
                    ),
                );
                match ctx.gh.merge_pr(pr) {
                    Ok(()) => {
                        let view = ctx.gh.view_pr(pr)?;
                        let sha = view.merge_commit.map(|c| c.oid).unwrap_or_default();
                        append_run_log(
                            "info",
                            "merge_loop.succeeded",
                            json!({
                                "worker_id": ctx.identity.worker_id, "pr_number": pr, "attempt": attempt + 1, "merge_sha": sha
                            }),
                        );
                        return Ok(MergeLoopOutcome::Merged { sha });
                    }
                    Err(merge_err) => {
                        step(ctx, "MERGE", &format!("fallback merge failed: {merge_err}"));
                        if attempt + 1 >= MAX_MERGE_REMEDIATION {
                            return Ok(MergeLoopOutcome::Failed {
                                reason: format!(
                                    "merge failed after {} attempts: {}",
                                    MAX_MERGE_REMEDIATION, merge_err
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(MergeLoopOutcome::Failed {
        reason: "merge loop completed without resolution".to_string(),
    })
}

fn merge_main_and_push(
    ctx: &mut MergeLoopContext<'_>,
    branch: &str,
    attempt: u32,
) -> Result<(), GardenerError> {
    if ctx.git.abort_merge_if_in_progress()? {
        step(
            ctx,
            "REMEDIATE",
            "aborted stale merge in progress before attempting merge main",
        );
        append_run_log(
            "warn",
            "merge_loop.merge_from_main.stale_merge_aborted",
            json!({
                "worker_id": ctx.identity.worker_id,
                "pr_number": ctx.pr_number,
                "attempt": attempt + 1
            }),
        );
    }
    match ctx.git.try_merge_from_main() {
        Ok(RebaseResult::Clean) => {
            append_run_log(
                "info",
                "merge_loop.merge_from_main.clean",
                json!({
                    "worker_id": ctx.identity.worker_id, "pr_number": ctx.pr_number, "attempt": attempt + 1
                }),
            );
            step(ctx, "REMEDIATE", "merge-from-main clean, pushing");
            ctx.git.push_with_rebase_recovery(branch)?;
            Ok(())
        }
        Ok(RebaseResult::Conflict { stderr }) => {
            append_run_log(
                "warn",
                "merge_loop.merge_from_main.conflict",
                json!({
                    "worker_id": ctx.identity.worker_id, "pr_number": ctx.pr_number, "attempt": attempt + 1, "stderr": stderr
                }),
            );
            step(
                ctx,
                "REMEDIATE",
                "merge-from-main has conflicts, running agent resolution",
            );
            let conflict_tpl = merge_main_conflict_resolution_template();
            let conflict_result = run_agent_turn(agent_turn_from_ctx(ctx, Some(&conflict_tpl)))?;
            if conflict_result.terminal != AgentTerminal::Failure {
                step(
                    ctx,
                    "REMEDIATE",
                    "agent resolved conflicts, committing and pushing",
                );
                ctx.git.commit_all("fix: merge main into branch")?;
                ctx.git.push_with_rebase_recovery(branch)?;
                Ok(())
            } else {
                step(ctx, "REMEDIATE", "agent failed to resolve conflicts");
                Err(GardenerError::Process(
                    "agent failed to resolve merge conflicts".to_string(),
                ))
            }
        }
        Err(e) => {
            append_run_log(
                "warn",
                "merge_loop.merge_from_main.failed",
                json!({
                    "worker_id": ctx.identity.worker_id, "pr_number": ctx.pr_number, "attempt": attempt + 1, "error": e.to_string()
                }),
            );
            Err(e)
        }
    }
}
