use crate::agent::factory::AdapterFactory;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::gh::GhClient;
use crate::git::GitClient;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::PromptRegistry;
use crate::runtime::{ProcessRunner, ProductionClock, ProductionFileSystem};
use crate::types::{RuntimeScope, WorkerState};
use crate::worker::{execute_merge_phase, MergeRequest, WorkerStreamEvent};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;

pub use crate::worker::{MAX_MERGE_REMEDIATION, MERGEABILITY_POLL_INTERVAL, MERGEABILITY_POLL_MAX};

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

pub fn run_merge_loop(ctx: &mut MergeLoopContext<'_>) -> Result<MergeLoopOutcome, GardenerError> {
    append_run_log(
        "info",
        "merge_loop.bridge.started",
        json!({
            "worker_id": ctx.identity.worker_id,
            "pr_number": ctx.pr_number,
            "worktree_path": ctx.worktree_path.display().to_string(),
            "delegated_to_merge_worker": true
        }),
    );
    // Keep the merge-loop API, but run the merge worker's canonical implementation.
    let req = MergeRequest {
        slot_idx: 0,
        task_id: format!("merge-pr-{}", ctx.pr_number),
        task_summary: ctx.task_summary.to_string(),
        attempt_count: ctx.attempt_count,
        worker_id: ctx.identity.worker_id.clone(),
        session_id: ctx.identity.session.session_id.clone(),
        worktree_path: ctx.worktree_path.to_path_buf(),
        branch: ctx.branch.to_string(),
        pr_number: ctx.pr_number,
        logs: Vec::new(),
        handoff_evidence_bundle: None,
    };

    let fs = ProductionFileSystem;
    let clock = ProductionClock;
    let on_worker_event = |event: WorkerStreamEvent| match event {
        WorkerStreamEvent::StateChanged { state, details, .. } => {
            if details.is_empty() {
                step(ctx, "STATE", &state);
            } else {
                step(ctx, "STATE", &format!("{state}: {details}"));
            }
        }
        WorkerStreamEvent::ToolCommand { command, .. } => {
            step(ctx, "TOOL", &command);
        }
    };

    let summary = execute_merge_phase(
        &req,
        ctx.cfg,
        ctx.process_runner,
        &fs,
        &clock,
        ctx.scope,
        Some(&on_worker_event),
    )?;

    if summary.final_state == WorkerState::Complete {
        let view = match ctx.gh.view_pr(ctx.pr_number) {
            Ok(view) => view,
            Err(_) => {
                // The merge worker may clean up the worktree; retry from repo root.
                let repo_gh = GhClient::new(ctx.process_runner, &ctx.scope.working_dir);
                repo_gh.view_pr(ctx.pr_number)?
            }
        };
        let sha = view.merge_commit.map(|c| c.oid).unwrap_or_default();
        append_run_log(
            "info",
            "merge_loop.bridge.succeeded",
            json!({
                "worker_id": ctx.identity.worker_id,
                "pr_number": ctx.pr_number,
                "merge_sha": sha
            }),
        );
        step(ctx, "DONE", &format!("merged (sha={sha})"));
        return Ok(MergeLoopOutcome::Merged { sha });
    }

    let reason = summary.failure_reason.unwrap_or_else(|| {
        format!(
            "merge worker completed in non-success state: {}",
            summary.final_state.as_str()
        )
    });
    append_run_log(
        "warn",
        "merge_loop.bridge.failed",
        json!({
            "worker_id": ctx.identity.worker_id,
            "pr_number": ctx.pr_number,
            "final_state": summary.final_state.as_str(),
            "reason": reason
        }),
    );
    step(ctx, "FAILED", &reason);
    Ok(MergeLoopOutcome::Failed { reason })
}
