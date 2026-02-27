use crate::agent::{validate_model, AdapterCapabilities, AdapterContext, AgentAdapter};
use crate::errors::GardenerError;
use crate::protocol::{map_codex_event, parse_jsonl, AgentTerminal, StepResult};
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
        Ok(AdapterCapabilities {
            backend: Some(AgentKind::Codex),
            version,
            supports_json: text.contains("--json"),
            supports_stream_json: false,
            supports_output_schema: text.contains("--output-schema"),
            supports_output_last_message: text.contains("--output-last-message")
                || text.contains(" -o "),
            supports_max_turns: text.contains("--max-turns"),
            supports_listen_stdio: text.contains("--listen stdio://") || text.contains("websocket"),
            supports_stdin_prompt: true,
        })
    }

    fn execute(
        &self,
        process_runner: &dyn ProcessRunner,
        context: &AdapterContext,
        prompt: &str,
    ) -> Result<StepResult, GardenerError> {
        validate_model(&context.model)?;

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

        let handle = process_runner.spawn(ProcessRequest {
            program: "codex".to_string(),
            args,
            cwd: Some(context.cwd.clone()),
        })?;

        let output = process_runner.wait(handle)?;
        if output.exit_code != 0 {
            return Err(GardenerError::Process(output.stderr));
        }
        let raw_events = parse_jsonl(&output.stdout)?;
        let events = raw_events.iter().map(map_codex_event).collect::<Vec<_>>();

        if let Some(failed) = raw_events.iter().find(|ev| {
            ev.get("type") == Some(&json!("turn.failed")) || ev.get("type") == Some(&json!("error"))
        }) {
            return Ok(StepResult {
                terminal: AgentTerminal::Failure,
                events,
                payload: failed.clone(),
                diagnostics: output
                    .stderr
                    .lines()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            });
        }

        let done = raw_events
            .iter()
            .rev()
            .find(|event| event.get("type") == Some(&json!("turn.completed")))
            .cloned()
            .ok_or_else(|| GardenerError::Process("missing turn.completed event".to_string()))?;

        let payload = done.get("result").cloned().unwrap_or(Value::Null);
        Ok(StepResult {
            terminal: AgentTerminal::Success,
            events,
            payload,
            diagnostics: output
                .stderr
                .lines()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        })
    }
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
            .execute(&runner, &context(), "prompt")
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
            .execute(&runner, &context(), "prompt")
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
            .execute(&runner, &context(), "prompt")
            .expect_err("must fail");
        assert!(format!("{err}").contains("missing turn.completed"));
    }
}
