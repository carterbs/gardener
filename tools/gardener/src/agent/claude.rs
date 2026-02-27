use crate::agent::{validate_model, AdapterCapabilities, AdapterContext, AgentAdapter};
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::protocol::{map_claude_event, AgentEvent, AgentTerminal, StepResult};
use crate::runtime::{ProcessRequest, ProcessRunner};
use crate::types::AgentKind;
use serde_json::{json, Value};

pub struct ClaudeAdapter;

impl AgentAdapter for ClaudeAdapter {
    fn backend(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn probe_capabilities(
        &self,
        process_runner: &dyn ProcessRunner,
    ) -> Result<AdapterCapabilities, GardenerError> {
        append_run_log(
            "debug",
            "adapter.claude.probe_capabilities.started",
            json!({}),
        );
        let help = process_runner.run(ProcessRequest {
            program: "claude".to_string(),
            args: vec!["--help".to_string()],
            cwd: None,
        })?;

        let version = process_runner
            .run(ProcessRequest {
                program: "claude".to_string(),
                args: vec!["--version".to_string()],
                cwd: None,
            })
            .ok()
            .map(|out| out.stdout.trim().to_string())
            .filter(|v| !v.is_empty());

        let text = format!("{}\n{}", help.stdout, help.stderr);

        let caps = AdapterCapabilities {
            backend: Some(AgentKind::Claude),
            version: version.clone(),
            supports_json: false,
            supports_stream_json: text.contains("--output-format"),
            supports_output_schema: false,
            supports_output_last_message: false,
            supports_max_turns: text.contains("--max-turns"),
            supports_listen_stdio: false,
            supports_stdin_prompt: false,
        };
        append_run_log(
            "info",
            "adapter.claude.probe_capabilities.completed",
            json!({
                "version": version,
                "supports_stream_json": caps.supports_stream_json,
                "supports_max_turns": caps.supports_max_turns
            }),
        );
        Ok(caps)
    }

    fn execute(
        &self,
        process_runner: &dyn ProcessRunner,
        context: &AdapterContext,
        prompt: &str,
        mut on_event: Option<&mut dyn FnMut(&AgentEvent)>,
    ) -> Result<StepResult, GardenerError> {
        validate_model(&context.model)?;
        append_run_log(
            "info",
            "adapter.claude.turn_start",
            json!({
                "worker_id": context.worker_id,
                "session_id": context.session_id,
                "sandbox_id": context.sandbox_id,
                "backend": "claude",
                "model": context.model,
                "cwd": context.cwd.display().to_string(),
                "prompt_version": context.prompt_version,
                "context_manifest_hash": context.context_manifest_hash,
                "max_turns": context.max_turns
            }),
        );

        let mut args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--model".to_string(),
            context.model.clone(),
        ];
        if let Some(turns) = context.max_turns {
            args.push("--max-turns".to_string());
            args.push(turns.to_string());
        }

        append_run_log(
            "debug",
            "adapter.claude.process_spawn",
            json!({
                "worker_id": context.worker_id,
                "session_id": context.session_id,
                "backend": "claude",
                "model": context.model,
                "program": "claude",
                "cwd": context.cwd.display().to_string()
            }),
        );
        let handle = process_runner.spawn(ProcessRequest {
            program: "claude".to_string(),
            args,
            cwd: Some(context.cwd.clone()),
        })?;

        let mut raw_events = Vec::new();
        let mut stdout_diagnostics = Vec::new();
        let mut stderr_diagnostics = Vec::new();
        let mut on_stdout_line = |line: &str| {
            if line.trim().is_empty() {
                return;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(raw) => {
                    let event = map_claude_event(&raw);
                    let kind = format!("{:?}", event.kind);
                    let raw_type = event.raw_type.clone();
                    let command = extract_action_command(&event.payload);
                    append_run_log(
                        "debug",
                        "adapter.claude.event",
                        json!({
                            "worker_id": context.worker_id,
                            "session_id": context.session_id,
                            "backend": "claude",
                            "model": context.model,
                            "kind": kind,
                            "raw_type": raw_type,
                            "command": command,
                            "payload": event.payload.clone()
                        }),
                    );
                    if let Some(sink) = on_event.as_deref_mut() {
                        sink(&event);
                    }
                    raw_events.push(raw);
                }
                Err(err) => {
                    append_run_log(
                        "warn",
                        "adapter.claude.stdout_non_json",
                        json!({
                            "worker_id": context.worker_id,
                            "session_id": context.session_id,
                            "backend": "claude",
                            "model": context.model,
                            "error": err.to_string(),
                            "line": line
                        }),
                    );
                    stdout_diagnostics
                        .push(format!("stdout non-json line ignored: {err}; line={line}"));
                }
            }
        };
        let mut on_stderr_line = |line: &str| {
            if line.trim().is_empty() {
                return;
            }
            append_run_log(
                "warn",
                "adapter.claude.stderr_line",
                json!({
                    "worker_id": context.worker_id,
                    "session_id": context.session_id,
                    "backend": "claude",
                    "model": context.model,
                    "line": line
                }),
            );
            stderr_diagnostics.push(line.to_string());
        };
        let output = process_runner.wait_with_line_stream(
            handle,
            &mut on_stdout_line,
            &mut on_stderr_line,
        )?;
        let mut diagnostics = stderr_diagnostics;
        diagnostics.extend(stdout_diagnostics);
        let events = raw_events.iter().map(map_claude_event).collect::<Vec<_>>();

        if let Some(terminal_result) = raw_events
            .iter()
            .rev()
            .find(|event| event.get("type") == Some(&json!("result")))
        {
            let payload = terminal_result
                .get("result")
                .cloned()
                .unwrap_or(Value::Null);
            let subtype = terminal_result
                .get("subtype")
                .and_then(Value::as_str)
                .unwrap_or("");
            let terminal = if subtype == "success" {
                AgentTerminal::Success
            } else {
                AgentTerminal::Failure
            };
            append_run_log(
                if terminal == AgentTerminal::Success {
                    "info"
                } else {
                    "error"
                },
                "adapter.claude.terminal_result",
                json!({
                    "worker_id": context.worker_id,
                    "session_id": context.session_id,
                    "backend": "claude",
                    "model": context.model,
                    "subtype": subtype,
                    "event_count": raw_events.len(),
                    "stderr_line_count": diagnostics.len(),
                    "exit_code": output.exit_code
                }),
            );
            return Ok(StepResult {
                terminal,
                events,
                payload,
                diagnostics,
            });
        }

        if output.exit_code != 0 {
            let mut reason = output.stderr.trim().to_string();
            if reason.is_empty() {
                reason = format!("claude exited with status {}", output.exit_code);
            }
            append_run_log(
                "error",
                "adapter.claude.turn_process_error",
                json!({
                    "worker_id": context.worker_id,
                    "session_id": context.session_id,
                    "backend": "claude",
                    "model": context.model,
                    "exit_code": output.exit_code,
                    "error": reason
                }),
            );
            return Err(GardenerError::Process(reason));
        }

        append_run_log(
            "error",
            "adapter.claude.turn_missing_terminal_event",
            json!({
                "worker_id": context.worker_id,
                "session_id": context.session_id,
                "backend": "claude",
                "model": context.model,
                "event_count": raw_events.len(),
                "stderr_line_count": diagnostics.len(),
                "exit_code": output.exit_code
            }),
        );
        Err(GardenerError::Process(
            "missing terminal result event".to_string(),
        ))
    }
}

fn extract_action_command(payload: &Value) -> Option<String> {
    payload
        .get("command")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            payload
                .get("input")
                .and_then(|input| input.get("command"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::ClaudeAdapter;
    use crate::agent::{AdapterContext, AgentAdapter};
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use std::path::PathBuf;

    fn context() -> AdapterContext {
        AdapterContext {
            worker_id: "w".to_string(),
            session_id: "s".to_string(),
            sandbox_id: "x".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            cwd: PathBuf::from("/repo"),
            prompt_version: "v1".to_string(),
            context_manifest_hash: "hash".to_string(),
            output_schema: None,
            output_file: None,
            permissive_mode: true,
            max_turns: Some(4),
        }
    }

    #[test]
    fn parses_ndjson_and_extracts_terminal_result_payload() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"message_start\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"result\":{\"ok\":true}}\n".to_string(),
            stderr: "warn\n".to_string(),
        }));
        let adapter = ClaudeAdapter;
        let result = adapter
            .execute(&runner, &context(), "prompt", None)
            .expect("success");
        assert_eq!(result.payload["ok"], true);
        assert_eq!(runner.spawned()[0].program, "claude");
        assert!(runner.spawned()[0].args.contains(&"-p".to_string()));
    }

    #[test]
    fn probe_detects_supported_flags() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "Usage: claude --output-format stream-json --max-turns\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "claude 1.2.3\n".to_string(),
            stderr: String::new(),
        }));

        let adapter = ClaudeAdapter;
        let caps = adapter.probe_capabilities(&runner).expect("caps");
        assert!(caps.supports_stream_json);
        assert!(caps.supports_max_turns);
        assert_eq!(caps.version.as_deref(), Some("claude 1.2.3"));
    }

    #[test]
    fn missing_success_event_is_rejected() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"message_start\"}\n".to_string(),
            stderr: String::new(),
        }));
        let adapter = ClaudeAdapter;
        let err = adapter
            .execute(&runner, &context(), "prompt", None)
            .expect_err("must fail");
        assert!(format!("{err}").contains("missing terminal result event"));
    }

    #[test]
    fn ignores_non_json_stdout_lines_when_terminal_result_exists() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "NOTICE startup\n{\"type\":\"result\",\"subtype\":\"success\",\"result\":{\"ok\":true}}\n"
                .to_string(),
            stderr: String::new(),
        }));
        let adapter = ClaudeAdapter;
        let result = adapter
            .execute(&runner, &context(), "prompt", None)
            .expect("success");
        assert_eq!(result.payload["ok"], true);
        assert!(result
            .diagnostics
            .iter()
            .any(|line| line.contains("stdout non-json line ignored")));
    }
}
