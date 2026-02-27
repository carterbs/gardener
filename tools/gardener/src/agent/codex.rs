use crate::agent::{validate_model, AdapterCapabilities, AdapterContext, AgentAdapter};
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::protocol::{map_codex_event, AgentEvent, AgentTerminal, StepResult};
use crate::runtime::{ProcessRequest, ProcessRunner};
use crate::types::AgentKind;
use serde_json::{json, Value};

pub struct CodexAdapter;

impl AgentAdapter for CodexAdapter {
    fn backend(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn probe_capabilities(
        &self,
        process_runner: &dyn ProcessRunner,
    ) -> Result<AdapterCapabilities, GardenerError> {
        append_run_log(
            "debug",
            "adapter.codex.probe_capabilities.started",
            json!({}),
        );
        let help = process_runner.run(ProcessRequest {
            program: "codex".to_string(),
            args: vec!["--help".to_string()],
            cwd: None,
        })?;

        let version = process_runner
            .run(ProcessRequest {
                program: "codex".to_string(),
                args: vec!["--version".to_string()],
                cwd: None,
            })
            .ok()
            .map(|out| out.stdout.trim().to_string())
            .filter(|v| !v.is_empty());

        let text = format!("{}\n{}", help.stdout, help.stderr);
        let caps = AdapterCapabilities {
            backend: Some(AgentKind::Codex),
            version: version.clone(),
            supports_json: text.contains("--json"),
            supports_stream_json: false,
            supports_output_schema: text.contains("--output-schema"),
            supports_output_last_message: text.contains("--output-last-message")
                || text.contains(" -o "),
            supports_max_turns: text.contains("--max-turns"),
            supports_listen_stdio: text.contains("--listen stdio://") || text.contains("websocket"),
            supports_stdin_prompt: true,
        };
        append_run_log(
            "info",
            "adapter.codex.probe_capabilities.completed",
            json!({
                "version": version,
                "supports_json": caps.supports_json,
                "supports_output_schema": caps.supports_output_schema,
                "supports_output_last_message": caps.supports_output_last_message,
                "supports_max_turns": caps.supports_max_turns,
                "supports_listen_stdio": caps.supports_listen_stdio
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
            "adapter.codex.turn_start",
            json!({
                "worker_id": context.worker_id,
                "session_id": context.session_id,
                "sandbox_id": context.sandbox_id,
                "backend": "codex",
                "model": context.model,
                "cwd": context.cwd.display().to_string(),
                "prompt_version": context.prompt_version,
                "context_manifest_hash": context.context_manifest_hash,
                "output_schema": context.output_schema.as_ref().map(|p| p.display().to_string()),
                "output_file": context.output_file.as_ref().map(|p| p.display().to_string())
            }),
        );

        let output_file = context
            .output_file
            .clone()
            .unwrap_or_else(|| context.cwd.join(".cache/gardener/codex-last-message.json"));

        let mut args = vec![
            "exec".to_string(),
            "--json".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "--model".to_string(),
            context.model.clone(),
            "-C".to_string(),
            context.cwd.display().to_string(),
            "-o".to_string(),
            output_file.display().to_string(),
        ];

        if let Some(schema) = &context.output_schema {
            args.push("--output-schema".to_string());
            args.push(schema.display().to_string());
        }

        args.push(prompt.to_string());

        append_run_log(
            "debug",
            "adapter.codex.process_spawn",
            json!({
                "worker_id": context.worker_id,
                "session_id": context.session_id,
                "backend": "codex",
                "model": context.model,
                "program": "codex",
                "cwd": context.cwd.display().to_string(),
                "output_file": context.output_file.as_ref().map(|p| p.display().to_string())
            }),
        );
        let handle = process_runner.spawn(ProcessRequest {
            program: "codex".to_string(),
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
                    let event = map_codex_event(&raw);
                    let kind = format!("{:?}", event.kind);
                    let raw_type = event.raw_type.clone();
                    let command = extract_action_command(&event.payload);
                    append_run_log(
                        "debug",
                        "adapter.codex.event",
                        json!({
                            "worker_id": context.worker_id,
                            "session_id": context.session_id,
                            "backend": "codex",
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
                        "adapter.codex.stdout_non_json",
                        json!({
                            "worker_id": context.worker_id,
                            "session_id": context.session_id,
                            "backend": "codex",
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
                "adapter.codex.stderr_line",
                json!({
                    "worker_id": context.worker_id,
                    "session_id": context.session_id,
                    "backend": "codex",
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
        let events = raw_events.iter().map(map_codex_event).collect::<Vec<_>>();

        if let Some(failed) = raw_events.iter().find(|ev| {
            ev.get("type") == Some(&json!("turn.failed")) || ev.get("type") == Some(&json!("error"))
        }) {
            let failure_type = failed
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let failure_reason = failed
                .get("reason")
                .or_else(|| failed.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("");
            append_run_log(
                "error",
                "adapter.codex.turn_failure_event",
                json!({
                    "worker_id": context.worker_id,
                    "session_id": context.session_id,
                    "backend": "codex",
                    "model": context.model,
                    "failure_type": failure_type,
                    "failure_reason": failure_reason,
                    "event_count": raw_events.len(),
                    "stderr_line_count": diagnostics.len()
                }),
            );
            return Ok(StepResult {
                terminal: AgentTerminal::Failure,
                events,
                payload: failed.clone(),
                diagnostics,
            });
        }

        if let Some(done) = raw_events
            .iter()
            .rev()
            .find(|event| event.get("type") == Some(&json!("turn.completed")))
        {
            append_run_log(
                "info",
                "adapter.codex.turn_completed_event",
                json!({
                    "worker_id": context.worker_id,
                    "session_id": context.session_id,
                    "backend": "codex",
                    "model": context.model,
                    "event_count": raw_events.len(),
                    "stderr_line_count": diagnostics.len(),
                    "exit_code": output.exit_code
                }),
            );
            let payload = done.get("result").cloned().unwrap_or(Value::Null);
            return Ok(StepResult {
                terminal: AgentTerminal::Success,
                events,
                payload,
                diagnostics,
            });
        }

        if output.exit_code != 0 {
            let mut reason = output.stderr.trim().to_string();
            if reason.is_empty() {
                reason = format!("codex exited with status {}", output.exit_code);
            }
            append_run_log(
                "error",
                "adapter.codex.turn_process_error",
                json!({
                    "worker_id": context.worker_id,
                    "session_id": context.session_id,
                    "backend": "codex",
                    "model": context.model,
                    "exit_code": output.exit_code,
                    "error": reason
                }),
            );
            return Err(GardenerError::Process(reason));
        }

        append_run_log(
            "error",
            "adapter.codex.turn_missing_terminal_event",
            json!({
                "worker_id": context.worker_id,
                "session_id": context.session_id,
                "backend": "codex",
                "model": context.model,
                "event_count": raw_events.len(),
                "stderr_line_count": diagnostics.len(),
                "exit_code": output.exit_code
            }),
        );
        Err(GardenerError::Process(
            "missing turn.completed or turn.failed event".to_string(),
        ))
    }
}

fn extract_action_command(payload: &Value) -> Option<String> {
    payload
        .get("item")
        .and_then(|item| item.get("command"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            payload
                .get("command")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::CodexAdapter;
    use crate::agent::{AdapterContext, AgentAdapter};
    use crate::protocol::AgentTerminal;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use std::path::PathBuf;

    fn context() -> AdapterContext {
        AdapterContext {
            worker_id: "w".to_string(),
            session_id: "s".to_string(),
            sandbox_id: "x".to_string(),
            model: "gpt-5-codex".to_string(),
            cwd: PathBuf::from("/repo"),
            prompt_version: "v1".to_string(),
            context_manifest_hash: "hash".to_string(),
            output_schema: Some(PathBuf::from("/repo/schema.json")),
            output_file: Some(PathBuf::from("/repo/out.json")),
            permissive_mode: true,
            max_turns: None,
        }
    }

    #[test]
    fn parses_jsonl_and_finishes_on_turn_completed() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"thread.started\"}\n{\"type\":\"turn.completed\",\"result\":{\"ok\":true}}\n".to_string(),
            stderr: "diag\n".to_string(),
        }));

        let adapter = CodexAdapter;
        let result = adapter
            .execute(&runner, &context(), "prompt", None)
            .expect("success");
        assert_eq!(result.terminal, AgentTerminal::Success);
        assert_eq!(result.payload["ok"], true);
        assert_eq!(runner.spawned()[0].program, "codex");
        assert!(runner.spawned()[0]
            .args
            .contains(&"--output-schema".to_string()));
    }

    #[test]
    fn turn_failed_is_failure_terminal() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.failed\",\"reason\":\"x\"}\n".to_string(),
            stderr: String::new(),
        }));

        let adapter = CodexAdapter;
        let result = adapter
            .execute(&runner, &context(), "prompt", None)
            .expect("parsed");
        assert_eq!(result.terminal, AgentTerminal::Failure);
    }

    #[test]
    fn probe_detects_json_and_schema_flags() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "Usage: codex --json --output-schema --output-last-message --listen stdio:// websocket\n".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "codex 9.9.9\n".to_string(),
            stderr: String::new(),
        }));
        let adapter = CodexAdapter;
        let caps = adapter.probe_capabilities(&runner).expect("caps");
        assert!(caps.supports_json);
        assert!(caps.supports_output_schema);
        assert!(caps.supports_output_last_message);
        assert!(caps.supports_listen_stdio);
    }

    #[test]
    fn missing_turn_completed_event_errors() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"thread.started\"}\n".to_string(),
            stderr: String::new(),
        }));
        let adapter = CodexAdapter;
        let err = adapter
            .execute(&runner, &context(), "prompt", None)
            .expect_err("must fail");
        assert!(format!("{err}").contains("missing turn.completed"));
    }

    #[test]
    fn ignores_non_json_stdout_lines_when_terminal_event_exists() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "INFO warmup\n{\"type\":\"turn.completed\",\"result\":{\"ok\":true}}\n"
                .to_string(),
            stderr: String::new(),
        }));
        let adapter = CodexAdapter;
        let result = adapter
            .execute(&runner, &context(), "prompt", None)
            .expect("must succeed");
        assert_eq!(result.terminal, AgentTerminal::Success);
        assert_eq!(result.payload["ok"], true);
        assert!(result
            .diagnostics
            .iter()
            .any(|line| line.contains("stdout non-json line ignored")));
    }

    #[test]
    fn returns_failure_terminal_when_failed_event_present_even_on_nonzero_exit() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: "{\"type\":\"turn.failed\",\"reason\":\"x\"}\n".to_string(),
            stderr: "nonzero".to_string(),
        }));
        let adapter = CodexAdapter;
        let result = adapter
            .execute(&runner, &context(), "prompt", None)
            .expect("failed terminal should be returned");
        assert_eq!(result.terminal, AgentTerminal::Failure);
    }
}
