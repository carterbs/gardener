use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::fsm::{
    DoingOutput, FsmSnapshot, MergingOutput, ReviewVerdict, ReviewingOutput, MAX_REVIEW_LOOPS,
};
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::output_envelope::{parse_typed_payload, END_MARKER, START_MARKER};
use crate::prompt_registry::PromptRegistry;
use crate::types::WorkerState;
use crate::understand_phase::classify_task;
use crate::worker::types::{TeardownReport, WorkerLogEvent, WorkerRunSummary};
use crate::worker_identity::WorkerIdentity;

pub(crate) fn execute_task_simulated(
    cfg: &AppConfig,
    worker_id: &str,
    _task_id: &str,
    task_summary: &str,
) -> Result<WorkerRunSummary, GardenerError> {
    append_run_log(
        "info",
        "worker.task.simulated.started",
        serde_json::json!({
            "worker_id": worker_id,
            "task_summary": task_summary
        }),
    );
    let registry = PromptRegistry::v1();
    let mut identity = WorkerIdentity::new(worker_id);
    let mut fsm = FsmSnapshot::default();
    let mut logs = Vec::new();

    let understand = crate::fsm::UnderstandOutput {
        task_type: classify_task(task_summary),
        reasoning: "deterministic keyword classifier".to_string(),
    };
    fsm.apply_understand(&understand, false)?;

    if fsm.state == WorkerState::Planning {
        fsm.transition(WorkerState::Doing)?;
    }

    let prepared = crate::agent_turn::prepare_prompt(
        cfg,
        &registry,
        &LearningLoop::default(),
        fsm.state,
        &identity.worker_id,
        task_summary,
        1,
        None,
    )?;
    logs.push(WorkerLogEvent {
        state: fsm.state,
        prompt_version: prepared.prompt_version,
        context_manifest_hash: prepared.context_manifest_hash,
    });

    let _doing_output: DoingOutput = parse_typed_payload(
        &format!(
            "{START_MARKER}{{\"schema_version\":1,\"state\":\"doing\",\"payload\":{{\"summary\":\"implementation complete\"}}}}{END_MARKER}"
        ),
        WorkerState::Doing,
    )?;

    fsm.on_doing_turn_completed()?;
    if fsm.state == WorkerState::Parked {
        append_run_log(
            "info",
            "worker.task.simulated.parked",
            serde_json::json!({
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
    fsm.transition(WorkerState::Reviewing)?;
    let reviewing_output = ReviewingOutput {
        verdict: ReviewVerdict::Approve,
        suggestions: vec![],
    };
    if reviewing_output.verdict == ReviewVerdict::NeedsChanges {
        if fsm.review_loops >= MAX_REVIEW_LOOPS {
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
        identity.begin_retry();
        fsm.transition(WorkerState::Doing)?;
    } else {
        fsm.transition(WorkerState::Merging)?;
    }

    let merge_output = MergingOutput {
        merged: true,
        merge_sha: Some("deadbeef".to_string()),
    };

    fsm.transition(WorkerState::Complete)?;

    let teardown = TeardownReport {
        merge_verified: merge_output.merged,
        session_torn_down: true,
        sandbox_torn_down: true,
        worktree_cleaned: true,
        state_cleared: true,
        main_updated: false,
    };

    append_run_log(
        "info",
        "worker.task.simulated.complete",
        serde_json::json!({
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
