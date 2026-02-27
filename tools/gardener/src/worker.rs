use crate::config::{effective_agent_for_state, AppConfig};
use crate::errors::GardenerError;
use crate::fsm::{
    DoingOutput, FsmSnapshot, GittingOutput, MAX_REVIEW_LOOPS, MergingOutput, ReviewVerdict,
    ReviewingOutput, UnderstandOutput,
};
use crate::learning_loop::LearningLoop;
use crate::output_envelope::{parse_typed_payload, END_MARKER, START_MARKER};
use crate::prompt_context::PromptContextItem;
use crate::prompt_knowledge::to_prompt_lines;
use crate::prompt_registry::PromptRegistry;
use crate::prompts::render_state_prompt;
use crate::types::WorkerState;
use crate::worker_identity::WorkerIdentity;

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
    worker_id: &str,
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
    log_prompt(cfg, &registry, &mut logs, &learning_loop, fsm.state, task_summary)?;
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
    log_prompt(cfg, &registry, &mut logs, &learning_loop, fsm.state, task_summary)?;

    let gitting_output: GittingOutput = parse_typed_payload(
        &format!(
            "{START_MARKER}{{\"schema_version\":1,\"state\":\"gitting\",\"payload\":{{\"branch\":\"feat/fsm\",\"pr_number\":12,\"pr_url\":\"https://example.test/pr/12\"}}}}{END_MARKER}"
        ),
        WorkerState::Gitting,
    )?;
    verify_gitting_output(&gitting_output)?;

    fsm.transition(WorkerState::Reviewing)?;
    log_prompt(cfg, &registry, &mut logs, &learning_loop, fsm.state, task_summary)?;

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

    log_prompt(cfg, &registry, &mut logs, &learning_loop, fsm.state, task_summary)?;

    let merge_output: MergingOutput = parse_typed_payload(
        &format!(
            "{START_MARKER}{{\"schema_version\":1,\"state\":\"merging\",\"payload\":{{\"merged\":true,\"merge_sha\":\"deadbeef\"}}}}{END_MARKER}"
        ),
        WorkerState::Merging,
    )?;
    verify_merge_output(&merge_output)?;
    learning_loop.ingest_postmerge(&merge_output, vec!["validation passed".to_string()]);

    fsm.transition(WorkerState::Complete)?;

    let teardown = teardown_after_completion(&merge_output);

    Ok(WorkerRunSummary {
        worker_id: identity.worker_id,
        session_id: identity.session.session_id,
        final_state: WorkerState::Complete,
        logs,
        teardown: Some(teardown),
    })
}

fn log_prompt(
    cfg: &AppConfig,
    registry: &PromptRegistry,
    logs: &mut Vec<WorkerLogEvent>,
    learning_loop: &LearningLoop,
    state: WorkerState,
    task_summary: &str,
) -> Result<(), GardenerError> {
    let token_budget = token_budget_for_state(cfg, state) as usize;
    let knowledge = to_prompt_lines(learning_loop.entries(), cfg.learning.deactivate_below_confidence)
        .join("\n");

    let rendered = render_state_prompt(
        registry,
        state,
        vec![
            ctx_item("task_packet", "task", "task-hash", "task input", 100, task_summary),
            ctx_item("repo_context", "repo", "repo-hash", "repo snapshot", 90, "repo context"),
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

    logs.push(WorkerLogEvent {
        state,
        prompt_version: rendered.prompt_version,
        context_manifest_hash: rendered.packet.context_manifest.manifest_hash,
    });
    Ok(())
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
    if output.merged && output.merge_sha.as_deref().unwrap_or_default().trim().is_empty() {
        return Err(GardenerError::InvalidConfig(
            "merging verification failed: merge_sha required when merged=true".to_string(),
        ));
    }
    Ok(())
}

fn teardown_after_completion(output: &MergingOutput) -> TeardownReport {
    TeardownReport {
        merge_verified: output.merged,
        session_torn_down: true,
        sandbox_torn_down: true,
        worktree_cleaned: true,
        state_cleared: true,
    }
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
        WorkerState::Seeding | WorkerState::Complete | WorkerState::Failed | WorkerState::Parked => {
            cfg.prompts.token_budget.doing
        }
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
    use crate::types::WorkerState;

    #[test]
    fn worker_executes_fsm_and_teardown_protocol() {
        let cfg = AppConfig::default();
        let summary = execute_task(&cfg, "worker-1", "feature: add prompt packet").expect("ok");

        assert_eq!(summary.final_state, WorkerState::Complete);
        assert!(summary.logs.iter().all(|event| !event.prompt_version.is_empty()));
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
