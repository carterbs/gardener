use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::protocol::{AgentEvent, StepResult};
use crate::runtime::{Clock, FileSystem, ProcessRunner};
use crate::types::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Write a prompt to a temp file and return its path.
/// Used to avoid exceeding OS `ARG_MAX` when prompts contain large payloads.
pub fn write_prompt_file(agent_id: &str, prompt: &str) -> Result<PathBuf, GardenerError> {
    let ts = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "gardener-{}-{}-{}.md",
        agent_id,
        ts,
        std::process::id()
    ));
    std::fs::write(&path, prompt)
        .map_err(|e| GardenerError::Process(format!("failed to write prompt file: {e}")))?;
    append_run_log(
        "debug",
        "agent.write_prompt_file",
        json!({
            "agent_id": agent_id,
            "path": path.display().to_string(),
            "prompt_bytes": prompt.len()
        }),
    );
    Ok(path)
}

pub mod claude;
pub mod codex;
pub mod factory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterContext {
    pub worker_id: String,
    pub session_id: String,
    pub sandbox_id: String,
    pub model: String,
    pub cwd: PathBuf,
    pub prompt_version: String,
    pub context_manifest_hash: String,
    pub output_schema: Option<PathBuf>,
    pub output_file: Option<PathBuf>,
    pub permissive_mode: bool,
    pub max_turns: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AdapterCapabilities {
    pub backend: Option<AgentKind>,
    pub version: Option<String>,
    pub supports_json: bool,
    pub supports_stream_json: bool,
    pub supports_output_schema: bool,
    pub supports_output_last_message: bool,
    pub supports_max_turns: bool,
    pub supports_listen_stdio: bool,
    pub supports_stdin_prompt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CapabilitySnapshot {
    pub generated_at_unix: u64,
    pub adapters: Vec<AdapterCapabilities>,
}

pub trait AgentAdapter: Send + Sync {
    fn backend(&self) -> AgentKind;
    fn probe_capabilities(
        &self,
        process_runner: &dyn ProcessRunner,
    ) -> Result<AdapterCapabilities, GardenerError>;
    fn execute(
        &self,
        process_runner: &dyn ProcessRunner,
        context: &AdapterContext,
        prompt: &str,
        on_event: Option<&mut dyn FnMut(&AgentEvent)>,
    ) -> Result<StepResult, GardenerError>;
}

pub fn probe_and_persist(
    adapters: &[&dyn AgentAdapter],
    process_runner: &dyn ProcessRunner,
    file_system: &dyn FileSystem,
    clock: &dyn Clock,
    cache_root: &Path,
) -> Result<CapabilitySnapshot, GardenerError> {
    append_run_log(
        "info",
        "agent.prerequisites.check.started",
        json!({
            "adapter_count": adapters.len(),
            "cache_root": cache_root.display().to_string()
        }),
    );
    let mut caps = Vec::new();
    for adapter in adapters {
        let backend = adapter.backend();
        append_run_log(
            "debug",
            "agent.prerequisites.adapter",
            json!({
                "backend": backend.as_str()
            }),
        );
        match adapter.probe_capabilities(process_runner) {
            Ok(cap) => {
                append_run_log(
                    "info",
                    "agent.prerequisites.adapter.ok",
                    json!({
                        "backend": backend.as_str(),
                        "version": cap.version,
                        "supports_stream_json": cap.supports_stream_json,
                        "supports_max_turns": cap.supports_max_turns,
                        "supports_output_schema": cap.supports_output_schema
                    }),
                );
                caps.push(cap);
            }
            Err(err) => {
                append_run_log(
                    "warn",
                    "agent.prerequisites.adapter.failed",
                    json!({
                        "backend": backend.as_str(),
                        "error": err.to_string()
                    }),
                );
                let backend_name = backend.as_str();
                return Err(GardenerError::Cli(format!(
                    "agent prerequisite check failed for '{backend_name}'; install the '{backend_name}' CLI on PATH"
                )));
            }
        }
    }

    let generated_at_unix = clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let snapshot = CapabilitySnapshot {
        generated_at_unix,
        adapters: caps,
    };

    let path = cache_root.join(".cache/gardener/adapter-capabilities.json");
    if let Some(parent) = path.parent() {
        file_system.create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| GardenerError::Process(format!("capability snapshot encode failed: {e}")))?;
    file_system.write_string(&path, &text)?;

    append_run_log(
        "info",
        "agent.probe.persisted",
        json!({
            "path": path.display().to_string(),
            "adapter_count": snapshot.adapters.len(),
            "generated_at_unix": generated_at_unix
        }),
    );

    Ok(snapshot)
}

pub fn validate_model(model: &str) -> Result<(), GardenerError> {
    if model.trim().is_empty() || model.trim() == "..." || model.eq_ignore_ascii_case("todo") {
        append_run_log(
            "error",
            "agent.model.invalid",
            json!({
                "model": model
            }),
        );
        return Err(GardenerError::InvalidConfig(
            "model value is invalid; configure a real model id".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        probe_and_persist, validate_model, AdapterCapabilities, AdapterContext, AgentAdapter,
    };
    use crate::agent::{claude::ClaudeAdapter, codex::CodexAdapter};
    use crate::protocol::{AgentTerminal, StepResult};
    use crate::runtime::{FakeClock, FakeFileSystem, FakeProcessRunner, FileSystem, ProcessOutput};
    use crate::types::AgentKind;
    use serde_json::json;
    use std::path::Path;

    struct TestAdapter;

    impl AgentAdapter for TestAdapter {
        fn backend(&self) -> AgentKind {
            AgentKind::Codex
        }

        fn probe_capabilities(
            &self,
            _process_runner: &dyn crate::runtime::ProcessRunner,
        ) -> Result<AdapterCapabilities, crate::errors::GardenerError> {
            Ok(AdapterCapabilities {
                backend: Some(AgentKind::Codex),
                supports_json: true,
                ..AdapterCapabilities::default()
            })
        }

        fn execute(
            &self,
            _process_runner: &dyn crate::runtime::ProcessRunner,
            _context: &AdapterContext,
            _prompt: &str,
            _on_event: Option<&mut dyn FnMut(&crate::protocol::AgentEvent)>,
        ) -> Result<StepResult, crate::errors::GardenerError> {
            Ok(StepResult {
                terminal: AgentTerminal::Success,
                events: vec![],
                payload: json!({}),
                diagnostics: vec![],
            })
        }
    }

    #[test]
    fn probe_persists_capability_snapshot() {
        let fs = FakeFileSystem::default();
        let runner = FakeProcessRunner::default();
        let clock = FakeClock::default();
        let adapter = TestAdapter;
        let snapshot = probe_and_persist(&[&adapter], &runner, &fs, &clock, Path::new("/repo"))
            .expect("snapshot");
        assert_eq!(snapshot.adapters.len(), 1);
        assert!(fs.exists(Path::new("/repo/.cache/gardener/adapter-capabilities.json")));
    }

    #[test]
    fn probe_persists_capability_snapshot_for_multiple_adapters() {
        let fs = FakeFileSystem::default();
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "codex usage".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "codex 1.0.0".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "claude --output-format".to_string(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "claude 2.0.0".to_string(),
            stderr: String::new(),
        }));
        let clock = FakeClock::default();
        let claude = ClaudeAdapter;
        let codex = CodexAdapter;
        let snapshot =
            probe_and_persist(&[&claude, &codex], &runner, &fs, &clock, Path::new("/repo"))
                .expect("snapshot");
        assert_eq!(snapshot.adapters.len(), 2);
        assert_eq!(runner.spawned().len(), 4);
    }

    #[test]
    fn probe_and_persist_reports_missing_cli_binary_as_cli_error() {
        struct FailingAdapter;

        impl AgentAdapter for FailingAdapter {
            fn backend(&self) -> AgentKind {
                AgentKind::Claude
            }

            fn probe_capabilities(
                &self,
                _process_runner: &dyn crate::runtime::ProcessRunner,
            ) -> Result<AdapterCapabilities, crate::errors::GardenerError> {
                Err(crate::errors::GardenerError::Process(
                    "process missing".to_string(),
                ))
            }

            fn execute(
                &self,
                _process_runner: &dyn crate::runtime::ProcessRunner,
                _context: &crate::agent::AdapterContext,
                _prompt: &str,
                _on_event: Option<&mut dyn FnMut(&crate::protocol::AgentEvent)>,
            ) -> Result<StepResult, crate::errors::GardenerError> {
                unreachable!()
            }
        }

        let fs = FakeFileSystem::default();
        let runner = FakeProcessRunner::default();
        let clock = FakeClock::default();
        let adapter = FailingAdapter;
        let err = probe_and_persist(&[&adapter], &runner, &fs, &clock, Path::new("/repo"))
            .expect_err("should fail");
        let message = format!("{err}");
        assert!(message.contains("agent prerequisite check failed for 'claude'"));
    }

    #[test]
    fn validate_model_rejects_placeholders() {
        for value in ["", "...", "todo", "TODO"] {
            let err = validate_model(value).expect_err("invalid");
            assert!(format!("{err}").contains("invalid"));
        }
        validate_model("gpt-5-codex").expect("valid");
    }
}
