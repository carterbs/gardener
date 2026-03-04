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
        "system" => AgentEventKind::ThreadStarted,
        "assistant" => AgentEventKind::Message,
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

// ── Shared agent-event summarization helpers ──────────────────────────

/// Summarize an `AgentEvent` into a short human-readable line for streaming UIs.
pub fn summarize_agent_event(event: &AgentEvent) -> Option<String> {
    match event.kind {
        AgentEventKind::ThreadStarted => Some("Agent session started".to_string()),
        AgentEventKind::TurnStarted => Some("Agent turn started".to_string()),
        AgentEventKind::TurnCompleted => Some("Agent turn completed".to_string()),
        AgentEventKind::TurnFailed => Some(format!(
            "Agent turn failed: {}",
            extract_event_label(&event.payload).unwrap_or_else(|| event.raw_type.clone())
        )),
        AgentEventKind::ToolCall => {
            let label =
                extract_event_label(&event.payload).unwrap_or_else(|| event.raw_type.clone());
            let command = extract_command_preview(&event.payload);
            Some(match command {
                Some(cmd) => format!("Agent activity: {label} started: {cmd}"),
                None => format!("Agent activity: {label} started"),
            })
        }
        AgentEventKind::ToolResult => {
            // Tool completions are noisy — the "started" line already shows the command
            None
        }
        AgentEventKind::Message => {
            extract_message_preview(&event.payload).map(|msg| format!("Agent thought: {msg}"))
        }
        AgentEventKind::Unknown => None,
    }
}

/// Extract a human-readable label from an event payload (tool name, error, etc.).
pub fn extract_event_label(payload: &Value) -> Option<String> {
    let candidates = [
        payload.pointer("/item/type").and_then(Value::as_str),
        payload.pointer("/item/name").and_then(Value::as_str),
        payload.pointer("/name").and_then(Value::as_str),
        payload.pointer("/tool_name").and_then(Value::as_str),
        payload.pointer("/reason").and_then(Value::as_str),
        payload.pointer("/error/message").and_then(Value::as_str),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(ToString::to_string)
}

/// Extract a command string from an event payload, stripping shell wrappers.
pub fn extract_command_preview(payload: &Value) -> Option<String> {
    let candidates = [
        payload.pointer("/item/command").and_then(Value::as_str),
        payload
            .pointer("/item/command_line")
            .and_then(Value::as_str),
        payload.pointer("/item/cmd").and_then(Value::as_str),
        payload.pointer("/command").and_then(Value::as_str),
        payload.pointer("/command_line").and_then(Value::as_str),
        payload.pointer("/cmd").and_then(Value::as_str),
        payload
            .pointer("/item/input/command")
            .and_then(Value::as_str),
        payload.pointer("/item/input/cmd").and_then(Value::as_str),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(strip_shell_wrapper)
        .map(|s| {
            let mut clipped = s;
            if clipped.len() > 120 {
                clipped.truncate(120);
                clipped.push_str("...");
            }
            clipped
        })
}

/// Extract a short message/thought preview from an event payload.
pub fn extract_message_preview(payload: &Value) -> Option<String> {
    let candidates = [
        payload.pointer("/delta/text").and_then(Value::as_str),
        payload.pointer("/text").and_then(Value::as_str),
        payload.pointer("/message").and_then(Value::as_str),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(|s| {
            let mut clipped = s.to_string();
            if clipped.len() > 120 {
                clipped.truncate(120);
                clipped.push_str("...");
            }
            clipped
        })
}

/// Strip shell wrappers like `/bin/zsh -lc '...'` or `/bin/zsh -lc "..."` from commands.
/// Handles both single and double quote variants, and both `-lc` and `-c` flags.
fn strip_shell_wrapper(cmd: &str) -> String {
    let trimmed = cmd.trim();
    // Prefixes to strip, in order of specificity
    let prefixes = [
        "/bin/zsh -lc ",
        "/bin/zsh -c ",
        "/bin/bash -lc ",
        "/bin/bash -c ",
    ];
    for prefix in &prefixes {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            // Strip matching outer quotes (single or double)
            let rest = rest.trim();
            if let Some(inner) = rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
                return inner.to_string();
            }
            if let Some(inner) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                return inner.to_string();
            }
            // No quotes or mismatched — return as-is without the prefix
            return rest.to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn extract_helpers_read_nested_and_fallback_fields() {
        assert_eq!(
            extract_event_label(&json!({"item": {"name": "test"}, "tool_name": "fallback"})),
            Some("test".to_string())
        );
        assert_eq!(
            extract_command_preview(&json!({"item": {"command_line": "cargo test --all-targets"}})),
            Some("cargo test --all-targets".to_string())
        );
        assert_eq!(
            extract_message_preview(&json!({"delta": {"text": "short payload"}})),
            Some("short payload".to_string())
        );
    }

    #[test]
    fn strip_shell_wrapper_single_quotes() {
        assert_eq!(
            extract_command_preview(&json!({"command": "/bin/zsh -lc 'cat Cargo.toml'"})),
            Some("cat Cargo.toml".to_string())
        );
        assert_eq!(
            extract_command_preview(&json!({"command": "/bin/bash -c 'ls -la'"})),
            Some("ls -la".to_string())
        );
    }

    #[test]
    fn strip_shell_wrapper_double_quotes() {
        // Real pattern from codex agent output
        assert_eq!(
            extract_command_preview(&json!({
                "command": "/bin/zsh -lc \"sed -n '1,200p' AGENTS.md && echo '---' && sed -n '1,200p' CLAUDE.md\""
            })),
            Some(
                "sed -n '1,200p' AGENTS.md && echo '---' && sed -n '1,200p' CLAUDE.md".to_string()
            )
        );
        assert_eq!(
            extract_command_preview(&json!({
                "command": "/bin/zsh -lc \"wc -l AGENTS.md CLAUDE.md\""
            })),
            Some("wc -l AGENTS.md CLAUDE.md".to_string())
        );
        assert_eq!(
            extract_command_preview(&json!({
                "command": "/bin/zsh -lc \"rg --files -g 'README*' -g 'Makefile' | head -n 200\""
            })),
            Some("rg --files -g 'README*' -g 'Makefile' | head -n 200".to_string())
        );
    }

    #[test]
    fn strip_shell_wrapper_no_wrapper_passthrough() {
        assert_eq!(
            extract_command_preview(&json!({"command": "pwd && ls -la"})),
            Some("pwd && ls -la".to_string())
        );
        assert_eq!(
            extract_command_preview(
                &json!({"command": "rg --files .github/workflows tools/gardener scripts | head -n 300"})
            ),
            Some("rg --files .github/workflows tools/gardener scripts | head -n 300".to_string())
        );
        assert_eq!(
            extract_command_preview(
                &json!({"command": "cat /Users/bradcarter/.codex/skills/research-codebase/SKILL.md"})
            ),
            Some("cat /Users/bradcarter/.codex/skills/research-codebase/SKILL.md".to_string())
        );
    }

    #[test]
    fn summarize_agent_event_handles_all_kinds() {
        assert_eq!(
            summarize_agent_event(&AgentEvent {
                protocol_version: 1,
                kind: AgentEventKind::ThreadStarted,
                raw_type: "thread.started".into(),
                payload: json!({}),
            }),
            Some("Agent session started".to_string())
        );
        assert!(summarize_agent_event(&AgentEvent {
            protocol_version: 1,
            kind: AgentEventKind::ToolCall,
            raw_type: "item.started".into(),
            payload: json!({"item": {"command": "echo hi"}}),
        })
        .as_deref()
        .expect("tool call preview")
        .contains("echo hi"));
        assert_eq!(
            summarize_agent_event(&AgentEvent {
                protocol_version: 1,
                kind: AgentEventKind::TurnFailed,
                raw_type: "turn.failed".into(),
                payload: json!({}),
            }),
            Some("Agent turn failed: turn.failed".to_string())
        );
        assert_eq!(
            summarize_agent_event(&AgentEvent {
                protocol_version: 1,
                kind: AgentEventKind::Unknown,
                raw_type: "unknown".into(),
                payload: json!({}),
            }),
            None
        );
    }
}
