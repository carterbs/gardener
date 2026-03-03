use crate::types::WorkerActivityState;
use crate::gh::{MergeStateStatus, Mergeable};
use crate::logging::append_run_log;
use crate::worker::types::WorkerStreamEvent;
use std::cell::RefCell;

type WorkerStateSink = Box<dyn Fn(&str, &str, &str)>;

thread_local! {
    static STATE_SINK: RefCell<Option<WorkerStateSink>> = const { RefCell::new(None) };
}

pub(crate) fn install_state_sink(sink: WorkerStateSink) {
    STATE_SINK.with(|cell| {
        *cell.borrow_mut() = Some(sink);
    });
}

pub(crate) fn clear_state_sink() {
    STATE_SINK.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

pub(crate) fn emit_adapter_tool_event(
    task_id: &str,
    on_event: Option<&dyn Fn(WorkerStreamEvent)>,
    event: &crate::protocol::AgentEvent,
) {
    let Some(on_event) = on_event else {
        return;
    };

    let raw_type = event.raw_type.as_str();
    let Some(command) = format_adapter_event_command(raw_type, &event.payload) else {
        return;
    };
    on_event(WorkerStreamEvent::ToolCommand {
        task_id: task_id.to_string(),
        command: truncate_utf8(&command, crate::worker::types::PROMPT_LINE_COMMAND_LIMIT),
    });
}

pub(crate) fn emit_worker_tool_command(
    task_id: &str,
    on_event: Option<&dyn Fn(WorkerStreamEvent)>,
    command: &str,
) {
    let Some(on_event) = on_event else {
        return;
    };
    on_event(WorkerStreamEvent::ToolCommand {
        task_id: task_id.to_string(),
        command: truncate_utf8(command, crate::worker::types::PROMPT_LINE_COMMAND_LIMIT),
    });
}

fn extract_payload_command(payload: &serde_json::Value) -> Option<String> {
    let normalize_command =
        |value: &serde_json::Value| value.as_str().map(|text| text.replace('\n', "\\n"));

    fn find_command(value: &serde_json::Value) -> Option<String> {
        let normalize_command =
            |value: &serde_json::Value| value.as_str().map(|text| text.replace('\n', "\\n"));

        if let Some(command) = value.get("command").and_then(normalize_command) {
            return Some(command);
        }

        let input = value.get("inputs").or_else(|| value.get("input"));
        if let Some(command) = input
            .and_then(|inputs| inputs.get("command").or_else(|| inputs.get("value")))
            .and_then(normalize_command)
        {
            return Some(command);
        }

        if let Some(command) = value.get("item").and_then(find_command) {
            return Some(command);
        }

        if let Some(message) = value.get("message") {
            if let Some(command) = message
                .get("content")
                .and_then(|content| content.as_array())
                .and_then(|commands| {
                    commands.iter().find_map(|command_entry| {
                        command_entry
                            .get("input")
                            .and_then(|input| input.get("command").or_else(|| input.get("value")))
                            .and_then(normalize_command)
                    })
                })
            {
                return Some(command);
            }
        }

        if let Some(message) = value.get("message").and_then(normalize_command) {
            return Some(message);
        }

        if let Some(text) = value.get("text").and_then(normalize_command) {
            return Some(text);
        }

        if let Some(content) = value.get("content").and_then(normalize_command) {
            return Some(content);
        }

        value.get("payload").and_then(find_command)
    }

    payload
        .get("command")
        .and_then(normalize_command)
        .or_else(|| find_command(payload))
}

fn format_adapter_event_command(event_type: &str, payload: &serde_json::Value) -> Option<String> {
    let message = extract_payload_command(payload)?;
    let kind = payload
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let raw_type = payload
        .get("raw_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !kind.is_empty() {
        Some(format!("{kind}: {message}"))
    } else if !raw_type.is_empty() {
        Some(format!("{raw_type}: {message}"))
    } else if event_type == "adapter.call" {
        Some(format!("call: {message}"))
    } else {
        Some(message)
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut cutoff = max_bytes.saturating_sub(3);
    while !value.is_char_boundary(cutoff) {
        cutoff = cutoff.saturating_sub(1);
    }
    format!("{}...", &value[..cutoff])
}

fn worker_state_details(state: &str, payload: Option<&serde_json::Value>) -> String {
    let Some(payload) = payload else {
        return String::new();
    };
    if state.is_empty() {
        return String::new();
    }
    let mut details = Vec::new();
    let mut push_detail = |name: &'static str, value: Option<&serde_json::Value>| {
        let Some(value) = value else {
            return;
        };
        if name == "next_check_in_secs" {
            if let Some(seconds) = value.as_u64() {
                details.push(format!("next_check_in={seconds}s"));
                return;
            }
        }
        let value = match value {
            serde_json::Value::String(s) if !s.is_empty() => s.to_string(),
            serde_json::Value::Null => String::new(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            other => serde_json::to_string(other).unwrap_or_else(|_| String::new()),
        };
        if !value.is_empty() {
            details.push(format!("{name}={value}"));
        }
    };
    push_detail("attempt", payload.get("attempt"));
    push_detail("pr_number", payload.get("pr_number"));
    push_detail("block_reason", payload.get("block_reason"));
    push_detail("mergeable", payload.get("mergeable"));
    push_detail("merge_state_status", payload.get("merge_state_status"));
    push_detail("next_check_in_secs", payload.get("next_check_in_secs"));
    if details.is_empty() {
        return String::new();
    }
    if state == "merge_polling" {
        return details.join(", ");
    }
    if state == "ci_failure_remediation" && !state.is_empty() {
        details.push(format!("state={state}"));
        return details.join(", ");
    }
    details.join(", ")
}

pub(crate) fn extract_failure_reason(payload: &serde_json::Value) -> Option<String> {
    let raw = payload
        .get("reason")
        .or_else(|| payload.get("message"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())?;
    // The message may be a JSON-encoded string like {"detail":"..."}
    if let Ok(inner) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(detail) = inner.get("detail").and_then(serde_json::Value::as_str) {
            return Some(detail.to_string());
        }
    }
    Some(raw.to_string())
}

pub(crate) fn emit_worker_activity_state(
    worker_id: &str,
    task_id: &str,
    state: WorkerActivityState,
    on_event: Option<&dyn Fn(WorkerStreamEvent)>,
) {
    emit_worker_activity_state_with(worker_id, task_id, state, serde_json::json!({}), on_event);
}

pub(crate) fn emit_worker_activity_state_with(
    worker_id: &str,
    task_id: &str,
    state: WorkerActivityState,
    details: serde_json::Value,
    on_event: Option<&dyn Fn(WorkerStreamEvent)>,
) {
    let mut payload = serde_json::json!({
        "worker_id": worker_id,
        "task_id": task_id,
        "state": state.as_str()
    });
    if let (serde_json::Value::Object(base), serde_json::Value::Object(extra)) =
        (&mut payload, &details)
    {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    let details_str = worker_state_details(state.as_str(), Some(&details));
    let sink_details = details_str.clone();
    if let Some(on_event) = on_event {
        on_event(WorkerStreamEvent::StateChanged {
            _task_id: task_id.to_string(),
            state: state.as_str().to_string(),
            details: details_str,
        });
    }
    STATE_SINK.with(|cell| {
        if let Some(sink) = cell.borrow().as_ref() {
            sink(state.as_str(), task_id, &sink_details);
        }
    });
    append_run_log("info", "worker.activity.state_changed", payload);
}

pub(crate) fn merge_polling_block_reason(
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

#[cfg(test)]
mod tests {
    use super::extract_failure_reason;

    #[test]
    fn extract_failure_reason_parses_nested_detail_field() {
        let detail = extract_failure_reason(
            &serde_json::json!({"message":"{\"detail\":\"merge conflicted\"}"}),
        );
        assert_eq!(detail.as_deref(), Some("merge conflicted"));

        let plain = extract_failure_reason(&serde_json::json!({"reason":"hook failed"}));
        assert_eq!(plain.as_deref(), Some("hook failed"));
        assert!(extract_failure_reason(&serde_json::json!({"other":123})).is_none());
    }
}
