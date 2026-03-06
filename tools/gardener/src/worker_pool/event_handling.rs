use super::scheduling::append_worker_command;
use super::PoolStreamEvent;
use crate::tui::{format_state_label, WorkerRow};
use std::sync::mpsc;
use std::time::Instant;

pub(super) fn apply_pool_stream_event(
    workers: &mut [WorkerRow],
    last_activity_pulse: &mut [Instant],
    event: PoolStreamEvent,
) -> bool {
    match event {
        PoolStreamEvent::ToolCommand {
            slot_idx,
            task_id,
            command,
        } => {
            let Some(worker) = workers.get_mut(slot_idx) else {
                return false;
            };
            if worker.task_id.as_deref() != Some(task_id.as_str()) {
                return false;
            }
            append_worker_command(worker, &command);
            worker.tool_line = command;
            if let Some(pulse) = last_activity_pulse.get_mut(slot_idx) {
                *pulse = Instant::now();
            }
            true
        }
        PoolStreamEvent::StateChanged {
            slot_idx,
            task_id,
            state,
            details,
        } => {
            let Some(worker) = workers.get_mut(slot_idx) else {
                return false;
            };
            if worker.task_id.as_deref() != Some(task_id.as_str()) {
                return false;
            }
            if !is_non_regressive_state_transition(&worker.state, &state) {
                return false;
            }
            let details = details.trim();
            let state_msg = if details.is_empty() {
                format!("state {state}")
            } else {
                format!("state {state}: {details}")
            };
            let state_label = format_state_label(&state);
            let tool_line = if details.is_empty() {
                state_label.as_str().to_string()
            } else {
                format!("{state_label} ({details})")
            };
            if worker.tool_line != tool_line || worker.state != state {
                worker.state = state.clone();
                worker.tool_line = tool_line;
                append_worker_command(worker, &state_msg);
                worker.breadcrumb = format!("state>{state}");
            }
            if let Some(pulse) = last_activity_pulse.get_mut(slot_idx) {
                *pulse = Instant::now();
            }
            true
        }
    }
}

pub(super) fn drain_pool_events(
    workers: &mut [WorkerRow],
    last_activity_pulse: &mut [Instant],
    event_sequence: &mut usize,
    event_rx: &mpsc::Receiver<PoolStreamEvent>,
) -> bool {
    let mut updated = false;
    while let Ok(event) = event_rx.try_recv() {
        if apply_pool_stream_event(workers, last_activity_pulse, event) {
            *event_sequence = event_sequence.saturating_add(1);
            updated = true;
        }
    }
    updated
}

pub(super) fn is_non_regressive_state_transition(current_state: &str, next_state: &str) -> bool {
    let current = normalize_worker_state_for_transition(current_state);
    let next = normalize_worker_state_for_transition(next_state);

    match current {
        "complete" | "failed" | "unresolved" | "parked" | "idle" => return false,
        _ => {}
    }
    if next == "unknown" {
        return false;
    }
    let current_rank = worker_state_rank(current);
    let next_rank = worker_state_rank(next);
    next_rank >= current_rank
}

fn worker_state_rank(state: &str) -> i32 {
    match state {
        "understand" => 0,
        "planning" => 1,
        "doing" => 2,
        "gitting" => 3,
        "reviewing" => 4,
        "merging" => 5,
        "complete" => 6,
        "failed" => 7,
        "unresolved" => 8,
        "parked" => 9,
        "idle" => 10,
        _ => 10,
    }
}

fn normalize_worker_state_for_transition(state: &str) -> &str {
    let normalized_state = state.trim().to_ascii_lowercase();
    let normalized_state = normalized_state
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .rfind(|part| !part.is_empty())
        .unwrap_or(normalized_state.as_str());

    match normalized_state {
        "init" | "boot" | "backlog_sync" | "working" | "seeding" => "understand",
        "claimed" | "starting" | "worktree_preparing" | "worktree_ready" => "understand",
        "commit" | "gitting_remediation" | "pr_creating" => "gitting",
        "merge_lock_waiting"
        | "merge_from_main"
        | "merge_lock_held"
        | "merge_polling"
        | "handoff"
        | "merge_remediation"
        | "post_merge_validation"
        | "teardown" => "merging",
        "understand" => "understand",
        "planning" => "planning",
        "doing" => "doing",
        "gitting" => "gitting",
        "reviewing" => "reviewing",
        "merging" => "merging",
        "complete" => "complete",
        "failed" => "failed",
        "unresolved" => "unresolved",
        "idle" => "idle",
        "parked" => "parked",
        _ => "unknown",
    }
}
