use crate::errors::GardenerError;
use serde::{Deserialize, Serialize};
use serde_json::{Deserializer, Value};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    ThreadStarted,
    TurnStarted,
    ToolCall,
    ToolResult,
    Message,
    TurnCompleted,
    TurnFailed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub protocol_version: u32,
    pub kind: AgentEventKind,
    pub raw_type: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTerminal {
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepResult {
    pub terminal: AgentTerminal,
    pub events: Vec<AgentEvent>,
    pub payload: Value,
    pub diagnostics: Vec<String>,
}

pub fn map_codex_event(raw: &Value) -> AgentEvent {
    let event_type = raw.get("type").and_then(Value::as_str).unwrap_or("unknown");

    let kind = match event_type {
        "thread.started" => AgentEventKind::ThreadStarted,
        "turn.started" => AgentEventKind::TurnStarted,
        "item.started" | "item.updated" => AgentEventKind::ToolCall,
        "item.completed" => AgentEventKind::ToolResult,
        "turn.completed" => AgentEventKind::TurnCompleted,
        "turn.failed" | "error" => AgentEventKind::TurnFailed,
        _ => AgentEventKind::Unknown,
    };

    AgentEvent {
        protocol_version: PROTOCOL_VERSION,
        kind,
        raw_type: event_type.to_string(),
        payload: raw.clone(),
    }
}

pub fn map_claude_event(raw: &Value) -> AgentEvent {
    let event_type = raw.get("type").and_then(Value::as_str).unwrap_or("unknown");

    let kind = match event_type {
        "message_start" => AgentEventKind::ThreadStarted,
        "content_block_start" => AgentEventKind::TurnStarted,
        "content_block_delta" => AgentEventKind::Message,
        "tool_use" => AgentEventKind::ToolCall,
        "tool_result" => AgentEventKind::ToolResult,
        "result" => {
            let subtype = raw.get("subtype").and_then(Value::as_str).unwrap_or("");
            if subtype == "success" {
                AgentEventKind::TurnCompleted
            } else {
                AgentEventKind::TurnFailed
            }
        }
        _ => AgentEventKind::Unknown,
    };

    AgentEvent {
        protocol_version: PROTOCOL_VERSION,
        kind,
        raw_type: event_type.to_string(),
        payload: raw.clone(),
    }
}

pub fn parse_jsonl(input: &str) -> Result<Vec<Value>, GardenerError> {
    let mut out = Vec::new();
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        out.extend(parse_json_records(line)?);
    }
    Ok(out)
}

pub fn parse_json_records(input: &str) -> Result<Vec<Value>, GardenerError> {
    let out = Deserializer::from_str(input)
        .into_iter::<Value>()
        .collect::<serde_json::Result<Vec<_>>>()
        .map_err(|err| {
            GardenerError::Process(format!(
                "invalid json stream: {err}; input={}",
                input.chars().take(256).collect::<String>(),
            ))
        })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{map_codex_event, parse_json_records, parse_jsonl, AgentEventKind};
    use serde_json::json;

    #[test]
    fn codex_unknown_events_are_retained() {
        let raw = json!({"type": "future.variant", "x": 1});
        let mapped = map_codex_event(&raw);
        assert_eq!(mapped.kind, AgentEventKind::Unknown);
        assert_eq!(mapped.raw_type, "future.variant");
    }

    #[test]
    fn jsonl_parser_rejects_malformed_lines() {
        let err = parse_jsonl("{\"type\":\"thread.started\"}\n{").expect_err("invalid");
        assert!(format!("{err}").contains("invalid json stream"));
    }

    #[test]
    fn parse_json_records_accepts_concatenated_events() {
        let events = parse_json_records(
            "{\"type\":\"thread.started\"}\n{\"type\":\"turn.completed\"}\n{\"type\":\"tool\"}",
        )
        .expect("should parse all");
        assert_eq!(events.len(), 3);
        assert_eq!(events[1]["type"], json!("turn.completed"));
    }

    #[test]
    fn parse_json_records_rejects_malformed_stream() {
        let err = parse_json_records("{\"type\":\"thread.started\"}\n{bad").expect_err("invalid");
        assert!(format!("{err}").contains("invalid json stream"));
    }
}
