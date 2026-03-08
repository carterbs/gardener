use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::gh::{FailedCheck, GhClient, MergeStateStatus, Mergeable};
use crate::git::{GitClient, RebaseResult};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::{
    ci_failure_remediation_template, merge_main_conflict_resolution_template, PromptRegistry,
};
use crate::protocol::AgentTerminal;
use crate::retry::{retry_with_backoff, RetryConfig};
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

pub const MAX_MERGE_REMEDIATION: u32 = 3;
pub const MERGEABILITY_POLL_MAX: u32 = 12;
pub const MERGEABILITY_POLL_INTERVAL: Duration = Duration::from_secs(5);

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
    pub pre_merge_validation: Option<&'a dyn Fn() -> Result<(), GardenerError>>,
    #[allow(clippy::type_complexity)]
    pub on_step: Option<&'a dyn Fn(&str, &str)>,
    pub on_agent_event: Option<&'a dyn Fn(&crate::protocol::AgentEvent)>,
}

#[derive(Debug)]
pub enum MergeLoopOutcome {
    Merged { sha: String },
    Failed { reason: String },
    Parked { reason: String },
}

impl MergeLoopOutcome {
    pub fn is_merged(&self) -> bool {
        matches!(self, MergeLoopOutcome::Merged { .. })
    }

    pub fn merge_sha(&self) -> Option<&str> {
        match self {
            MergeLoopOutcome::Merged { sha } => Some(sha),
            MergeLoopOutcome::Failed { .. } | MergeLoopOutcome::Parked { .. } => None,
        }
    }
}

fn step(ctx: &MergeLoopContext<'_>, label: &str, detail: &str) {
    if let Some(on_step) = ctx.on_step {
        on_step(label, detail);
    }
}

fn emit_agent_event(ctx: &MergeLoopContext<'_>, event: &crate::protocol::AgentEvent) {
    if let Some(on_agent_event) = ctx.on_agent_event {
        on_agent_event(event);
    }
}

/// Core merge loop: poll mergeability, run state machine, handle remediation.
///
/// Returns `MergeLoopOutcome::Merged` with the merge SHA on success, or
/// `Failed`/`Parked` with a reason. Pre-merge validation (if provided via
/// `ctx.pre_merge_validation`) runs before each `merge_pr` call in the
/// `Clean|HasHooks` arm.
pub fn run_merge_loop(ctx: &mut MergeLoopContext<'_>) -> Result<MergeLoopOutcome, GardenerError> {
    let pr = ctx.pr_number;
    let branch = ctx.branch.to_string();
    let worker_id = ctx.identity.worker_id.clone();

    for attempt in 0..MAX_MERGE_REMEDIATION {
        let status = retry_with_backoff(
            &RetryConfig {
                operation_name: "poll_mergeability",
                max_attempts: 2,
                ..Default::default()
            },
            || {
                ctx.gh
                    .poll_mergeability(pr, MERGEABILITY_POLL_MAX, MERGEABILITY_POLL_INTERVAL)
            },
        )?;

        // Check if the PR was already merged.
        if let Ok(pr_view) = ctx.gh.view_pr(pr) {
            if pr_view.state.eq_ignore_ascii_case("MERGED") || pr_view.merged_at.is_some() {
                append_run_log(
                    "info",
                    "merge_loop.already_merged",
                    json!({
                        "worker_id": worker_id,
                        "pr_number": pr,
                        "attempt": attempt + 1,
                        "merge_sha": pr_view.merge_commit.as_ref().map(|c| c.oid.clone())
                    }),
                );
                let sha = pr_view
                    .merge_commit
                    .as_ref()
                    .map(|c| c.oid.clone())
                    .unwrap_or_default();
                step(ctx, "MERGE", &format!("already merged (sha={sha})"));
                return Ok(MergeLoopOutcome::Merged { sha });
            }
        }

        let block_reason =
            merge_polling_block_reason(&status.mergeable, &status.merge_state_status);
        step(
            ctx,
            "POLL",
            &format!(
                "mergeable={:?} status={:?}{}",
                status.mergeable,
                status.merge_state_status,
                block_reason
                    .map(|r| format!(" block_reason={r}"))
                    .unwrap_or_default()
            ),
        );

        append_run_log(
            "info",
            "merge_loop.poll_result",
            json!({
                "worker_id": worker_id,
                "pr_number": pr,
                "attempt": attempt + 1,
                "mergeable": format!("{:?}", status.mergeable),
                "merge_state_status": format!("{:?}", status.merge_state_status)
            }),
        );

        match (&status.mergeable, &status.merge_state_status) {
            (Mergeable::Mergeable, MergeStateStatus::Clean | MergeStateStatus::HasHooks) => {
                // Pre-merge validation gate
                if let Some(validation_fn) = &ctx.pre_merge_validation {
                    step(ctx, "VALIDATE", "running pre-merge validation");
                    if let Err(err) = (validation_fn)() {
                        step(
                            ctx,
                            "VALIDATE",
                            &format!("pre-merge validation failed: {err}"),
                        );
                        append_run_log(
                            "error",
                            "merge_loop.pre_validation_failed",
                            json!({
                                "worker_id": worker_id,
                                "pr_number": pr,
                                "error": err.to_string()
                            }),
                        );
                        return Ok(MergeLoopOutcome::Failed {
                            reason: format!("pre-merge validation failed: {err}"),
                        });
                    }
                }

                step(ctx, "MERGE", &format!("gh pr merge {pr}"));
                match ctx.gh.merge_pr(pr) {
                    Ok(()) => {
                        let view = retry_with_backoff(
                            &RetryConfig {
                                operation_name: "view_pr_after_merge",
                                ..Default::default()
                            },
                            || ctx.gh.view_pr(pr),
                        )?;
                        let sha = view.merge_commit.map(|c| c.oid).unwrap_or_default();
                        append_run_log(
                            "info",
                            "merge_loop.merge_succeeded",
                            json!({
                                "worker_id": worker_id,
                                "pr_number": pr,
                                "attempt": attempt + 1,
                                "merge_sha": sha
                            }),
                        );
                        step(ctx, "MERGE", &format!("merged (sha={sha})"));
                        return Ok(MergeLoopOutcome::Merged { sha });
                    }
                    Err(merge_err) => {
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
            (_, MergeStateStatus::Behind) => {
                step(
                    ctx,
                    "REMEDIATE",
                    &format!("branch behind main (attempt {})", attempt + 1),
                );
                if let Err(e) =
                    merge_main_and_push(ctx, pr, &branch, &worker_id, attempt)
                {
                    step(ctx, "REMEDIATE", "merge from main failed, running agent");
                    ctx.learning_loop.ingest_failure(
                        WorkerState::Merging,
                        "merge from main failed, agent remediation needed",
                        vec![format!("error={e}")],
                    );
                    let on_adapter_event = |agent_event: &crate::protocol::AgentEvent| {
                        emit_agent_event(ctx, agent_event)
                    };
                    let remediation_result = run_agent_turn(AgentTurnInput {
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
                        prompt_override: None,
                        on_event: Some(&on_adapter_event),
                    })?;
                    if remediation_result.terminal == AgentTerminal::Failure
                        && attempt + 1 >= MAX_MERGE_REMEDIATION
                    {
                        let failure_reason =
                            extract_failure_reason(&remediation_result.payload);
                        return Ok(MergeLoopOutcome::Failed {
                            reason: failure_reason.unwrap_or_else(|| {
                                "merge from main remediation failed".to_string()
                            }),
                        });
                    }
                }
                continue;
            }
            (Mergeable::Conflicting, _) | (_, MergeStateStatus::Dirty) => {
                step(
                    ctx,
                    "REMEDIATE",
                    &format!("conflict/dirty (attempt {})", attempt + 1),
                );
                if let Err(e) =
                    merge_main_and_push(ctx, pr, &branch, &worker_id, attempt)
                {
                    step(ctx, "REMEDIATE", "conflict resolution failed, running agent");
                    ctx.learning_loop.ingest_failure(
                        WorkerState::Merging,
                        "conflict resolution failed, agent remediation needed",
                        vec![format!("error={e}")],
                    );
                    let on_adapter_event = |agent_event: &crate::protocol::AgentEvent| {
                        emit_agent_event(ctx, agent_event)
                    };
                    let remediation_result = run_agent_turn(AgentTurnInput {
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
                        prompt_override: None,
                        on_event: Some(&on_adapter_event),
                    })?;
                    if remediation_result.terminal == AgentTerminal::Failure
                        && attempt + 1 >= MAX_MERGE_REMEDIATION
                    {
                        let failure_reason =
                            extract_failure_reason(&remediation_result.payload);
                        return Ok(MergeLoopOutcome::Failed {
                            reason: failure_reason.unwrap_or_else(|| {
                                "conflict resolution remediation failed".to_string()
                            }),
                        });
                    }
                }
                continue;
            }
            (_, MergeStateStatus::Unstable) => {
                step(
                    ctx,
                    "REMEDIATE",
                    &format!("CI unstable (attempt {})", attempt + 1),
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
                ctx.learning_loop
                    .ingest_failure(WorkerState::Merging, "CI checks failed", evidence);
                let ci_tpl = ci_failure_remediation_template();
                let on_adapter_event = |agent_event: &crate::protocol::AgentEvent| {
                    emit_agent_event(ctx, agent_event)
                };
                let ci_result = run_agent_turn(AgentTurnInput {
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
                    prompt_override: Some(&ci_tpl),
                    on_event: Some(&on_adapter_event),
                })?;
                if ci_result.terminal == AgentTerminal::Failure
                    && attempt + 1 >= MAX_MERGE_REMEDIATION
                {
                    let failure_reason = extract_failure_reason(&ci_result.payload);
                    return Ok(MergeLoopOutcome::Failed {
                        reason: failure_reason
                            .unwrap_or_else(|| "CI failure remediation failed".to_string()),
                    });
                }
                continue;
            }
            (_, MergeStateStatus::Blocked) => {
                append_run_log(
                    "warn",
                    "merge_loop.blocked",
                    json!({
                        "worker_id": worker_id,
                        "pr_number": pr,
                        "attempt": attempt + 1,
                        "mergeable": format!("{:?}", status.mergeable),
                        "merge_state_status": format!("{:?}", status.merge_state_status)
                    }),
                );
                let failed_checks = ctx.gh.fetch_failed_checks(pr).unwrap_or_default();
                if !has_explicit_failed_checks(&failed_checks) {
                    if attempt + 1 >= MAX_MERGE_REMEDIATION {
                        return Ok(MergeLoopOutcome::Parked {
                            reason: "PR blocked by branch protection rules, parked for retry"
                                .to_string(),
                        });
                    }
                    continue;
                }
                step(
                    ctx,
                    "REMEDIATE",
                    &format!("blocked with failed checks (attempt {})", attempt + 1),
                );
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
                let ci_tpl = ci_failure_remediation_template();
                let on_adapter_event = |agent_event: &crate::protocol::AgentEvent| {
                    emit_agent_event(ctx, agent_event)
                };
                let ci_result = run_agent_turn(AgentTurnInput {
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
                    prompt_override: Some(&ci_tpl),
                    on_event: Some(&on_adapter_event),
                })?;
                if ci_result.terminal == AgentTerminal::Failure
                    && attempt + 1 >= MAX_MERGE_REMEDIATION
                {
                    let failure_reason = extract_failure_reason(&ci_result.payload);
                    return Ok(MergeLoopOutcome::Failed {
                        reason: failure_reason
                            .unwrap_or_else(|| "blocked CI remediation failed".to_string()),
                    });
                }
                continue;
            }
            _ => {
                append_run_log(
                    "warn",
                    "merge_loop.fallback_attempt",
                    json!({
                        "worker_id": worker_id,
                        "pr_number": pr,
                        "attempt": attempt + 1,
                        "mergeable": format!("{:?}", status.mergeable),
                        "merge_state_status": format!("{:?}", status.merge_state_status)
                    }),
                );
                // Run pre-merge validation in fallback arm too
                if let Some(validation_fn) = &ctx.pre_merge_validation {
                    step(ctx, "VALIDATE", "running pre-merge validation (fallback)");
                    if let Err(err) = (validation_fn)() {
                        append_run_log(
                            "error",
                            "merge_loop.pre_validation_failed",
                            json!({
                                "worker_id": worker_id,
                                "pr_number": pr,
                                "error": err.to_string()
                            }),
                        );
                        return Ok(MergeLoopOutcome::Failed {
                            reason: format!("pre-merge validation failed: {err}"),
                        });
                    }
                }
                step(ctx, "MERGE", &format!("gh pr merge {pr} (fallback)"));
                match ctx.gh.merge_pr(pr) {
                    Ok(()) => {
                        let view = retry_with_backoff(
                            &RetryConfig {
                                operation_name: "view_pr_after_merge",
                                ..Default::default()
                            },
                            || ctx.gh.view_pr(pr),
                        )?;
                        let sha = view.merge_commit.map(|c| c.oid).unwrap_or_default();
                        step(ctx, "MERGE", &format!("merged (sha={sha})"));
                        return Ok(MergeLoopOutcome::Merged { sha });
                    }
                    Err(merge_err) => {
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

    // Exhausted all attempts without merging or returning
    Ok(MergeLoopOutcome::Failed {
        reason: format!(
            "merge loop exhausted {MAX_MERGE_REMEDIATION} attempts without resolution"
        ),
    })
}

fn merge_main_and_push(
    ctx: &mut MergeLoopContext<'_>,
    pr: u64,
    branch: &str,
    worker_id: &str,
    attempt: u32,
) -> Result<(), GardenerError> {
    if ctx.git.abort_merge_if_in_progress()? {
        step(ctx, "REMEDIATE", "aborted stale in-progress merge");
        append_run_log(
            "warn",
            "merge_loop.merge_from_main.stale_merge_aborted",
            json!({
                "worker_id": worker_id,
                "pr_number": pr,
                "attempt": attempt + 1
            }),
        );
    }

    step(
        ctx,
        "REMEDIATE",
        "git fetch origin main && git merge origin/main",
    );
    match ctx.git.try_merge_from_main() {
        Ok(RebaseResult::Clean) => {
            append_run_log(
                "info",
                "merge_loop.merge_from_main.clean",
                json!({
                    "worker_id": worker_id,
                    "pr_number": pr,
                    "attempt": attempt + 1
                }),
            );
            step(ctx, "REMEDIATE", &format!("git push origin {branch}"));
            ctx.git.push_with_rebase_recovery(branch)?;
            Ok(())
        }
        Ok(RebaseResult::Conflict { stderr }) => {
            append_run_log(
                "warn",
                "merge_loop.merge_from_main.conflict",
                json!({
                    "worker_id": worker_id,
                    "pr_number": pr,
                    "attempt": attempt + 1,
                    "stderr": stderr
                }),
            );
            ctx.learning_loop.ingest_failure(
                WorkerState::Merging,
                "merge from main had conflicts",
                vec![format!("stderr={stderr}")],
            );
            let conflict_tpl = merge_main_conflict_resolution_template();
            let on_adapter_event = |agent_event: &crate::protocol::AgentEvent| {
                emit_agent_event(ctx, agent_event)
            };
            let conflict_result = run_agent_turn(AgentTurnInput {
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
                prompt_override: Some(&conflict_tpl),
                on_event: Some(&on_adapter_event),
            })?;
            if conflict_result.terminal != AgentTerminal::Failure {
                step(
                    ctx,
                    "REMEDIATE",
                    "conflict resolved, committing and pushing",
                );
                ctx.git.commit_all("fix: merge main into branch")?;
                ctx.git.push_with_rebase_recovery(branch)?;
                Ok(())
            } else {
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
                    "worker_id": worker_id,
                    "pr_number": pr,
                    "attempt": attempt + 1,
                    "error": e.to_string()
                }),
            );
            Err(e)
        }
    }
}

fn has_explicit_failed_checks(failed_checks: &[FailedCheck]) -> bool {
    !failed_checks.is_empty()
}

fn merge_polling_block_reason(
    mergeable: &Mergeable,
    merge_state_status: &MergeStateStatus,
) -> Option<&'static str> {
    match (mergeable, merge_state_status) {
        (Mergeable::Mergeable, MergeStateStatus::Clean | MergeStateStatus::HasHooks) => None,
        (Mergeable::Conflicting, _) => Some("merge conflicts detected"),
        (_, MergeStateStatus::Blocked) => {
            Some("blocked by branch protection rules or required checks")
        }
        (_, MergeStateStatus::Dirty) => Some("checks are still running"),
        (_, MergeStateStatus::Unstable) => Some("checks are failing or unstable"),
        (_, MergeStateStatus::Behind) => Some("branch is behind main"),
        (Mergeable::Unknown, _) => Some("mergeability is currently unknown"),
        _ => None,
    }
}

fn extract_failure_reason(payload: &serde_json::Value) -> Option<String> {
    let raw = payload
        .get("reason")
        .or_else(|| payload.get("message"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())?;
    if let Ok(inner) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(detail) = inner.get("detail").and_then(serde_json::Value::as_str) {
            return Some(detail.to_string());
        }
    }
    Some(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::{has_explicit_failed_checks, merge_polling_block_reason};
    use crate::gh::{FailedCheck, MergeStateStatus, Mergeable};

    #[test]
    fn blocked_state_without_explicit_failed_checks_is_not_remediated() {
        assert!(!has_explicit_failed_checks(&[]));
    }

    #[test]
    fn blocked_state_with_explicit_failed_checks_is_remediated() {
        let failed = vec![FailedCheck {
            name: "ci".to_string(),
            link: "https://github.com/example/repo/actions/runs/1/job/2".to_string(),
            log_snippet: "failing log".to_string(),
        }];
        assert!(has_explicit_failed_checks(&failed));
    }

    #[test]
    fn merge_polling_block_reason_maps_states() {
        assert!(merge_polling_block_reason(&Mergeable::Mergeable, &MergeStateStatus::Clean).is_none());
        assert_eq!(
            merge_polling_block_reason(&Mergeable::Conflicting, &MergeStateStatus::Clean),
            Some("merge conflicts detected")
        );
        assert_eq!(
            merge_polling_block_reason(&Mergeable::Mergeable, &MergeStateStatus::Blocked),
            Some("blocked by branch protection rules or required checks")
        );
        assert_eq!(
            merge_polling_block_reason(&Mergeable::Mergeable, &MergeStateStatus::Behind),
            Some("branch is behind main")
        );
    }
}
