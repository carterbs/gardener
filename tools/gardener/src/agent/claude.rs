use crate::agent::{validate_model, AdapterCapabilities, AdapterContext, AgentAdapter};
use crate::errors::GardenerError;
use crate::protocol::{map_claude_event, parse_jsonl, AgentTerminal, StepResult};
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

        Ok(AdapterCapabilities {
            backend: Some(AgentKind::Claude),
            version,
            supports_json: false,
            supports_stream_json: text.contains("--output-format"),
            supports_output_schema: false,
            supports_output_last_message: false,
            supports_max_turns: text.contains("--max-turns"),
            supports_listen_stdio: false,
            supports_stdin_prompt: false,
        })
    }

    fn execute(
        &self,
        process_runner: &dyn ProcessRunner,
        context: &AdapterContext,
        prompt: &str,
    ) -> Result<StepResult, GardenerError> {
        validate_model(&context.model)?;

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

        let handle = process_runner.spawn(ProcessRequest {
            program: "claude".to_string(),
            args,
            cwd: Some(context.cwd.clone()),
        })?;

        if context.cancel_requested {
            process_runner.kill(handle)?;
            return Err(GardenerError::Process(
                "claude adapter canceled: child terminated".to_string(),
            ));
        }

        let output = process_runner.wait(handle)?;
        if output.exit_code != 0 {
            return Err(GardenerError::Process(output.stderr));
        }

        let raw_events = parse_jsonl(&output.stdout)?;
        let events = raw_events.iter().map(map_claude_event).collect::<Vec<_>>();

        let terminal_result = raw_events
            .iter()
            .rev()
            .find(|event| {
                event.get("type") == Some(&json!("result"))
                    && event.get("subtype") == Some(&json!("success"))
            })
            .ok_or_else(|| GardenerError::Process("missing success result event".to_string()))?;

        let payload = terminal_result
            .get("result")
            .cloned()
            .unwrap_or(Value::Null);

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
            knowledge_refs: vec![],
            output_schema: None,
            output_file: None,
            permissive_mode: true,
            max_turns: Some(4),
            cancel_requested: false,
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
            .execute(&runner, &context(), "prompt")
            .expect("success");
        assert_eq!(result.payload["ok"], true);
        assert_eq!(runner.spawned()[0].program, "claude");
        assert!(runner.spawned()[0].args.contains(&"-p".to_string()));
    }

    #[test]
    fn cancel_path_kills_child() {
        let runner = FakeProcessRunner::default();
        let adapter = ClaudeAdapter;
        let mut ctx = context();
        ctx.cancel_requested = true;
        let err = adapter
            .execute(&runner, &ctx, "prompt")
            .expect_err("canceled");
        assert!(format!("{err}").contains("canceled"));
        assert_eq!(runner.kills(), vec![0]);
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
            .execute(&runner, &context(), "prompt")
            .expect_err("must fail");
        assert!(format!("{err}").contains("missing success result event"));
    }
}
