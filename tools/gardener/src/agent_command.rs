use serde_json::Value;

pub(crate) fn extract_agent_command(payload: &Value) -> Option<String> {
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
        payload.pointer("/input/command").and_then(Value::as_str),
        payload.pointer("/input/cmd").and_then(Value::as_str),
    ];

    for command in candidates {
        let command = command.map(str::trim);
        if let Some(command) = command.filter(|s| !s.is_empty()) {
            return Some(command.to_string());
        }
    }

    payload
        .get("item")
        .and_then(extract_agent_command)
        .or_else(|| payload.get("input").and_then(extract_agent_command))
        .or_else(|| payload.get("payload").and_then(extract_agent_command))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::extract_agent_command;

    #[test]
    fn extract_agent_command_prefers_nested_command_shapes() {
        assert_eq!(
            extract_agent_command(&json!({
                "item": { "command": "echo done" },
                "command": "ignored"
            })),
            Some("echo done".to_string())
        );

        assert_eq!(
            extract_agent_command(&json!({
                "item": {
                    "command_line": "cargo test"
                }
            })),
            Some("cargo test".to_string())
        );
    }

    #[test]
    fn extract_agent_command_reads_input_shapes() {
        assert_eq!(
            extract_agent_command(&json!({
                "item": {
                    "input": { "command": "git status" }
                }
            })),
            Some("git status".to_string())
        );

        assert_eq!(
            extract_agent_command(&json!({
                "payload": {
                    "command_line": "git diff"
                }
            })),
            Some("git diff".to_string())
        );
    }

    #[test]
    fn extract_agent_command_rejects_non_command_messages() {
        assert!(extract_agent_command(&json!({
            "message": "assistant thought",
            "text": "running analysis",
            "content": "tool output"
        }))
        .is_none());
    }
}
