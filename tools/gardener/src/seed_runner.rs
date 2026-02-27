use crate::agent::factory::AdapterFactory;
use crate::agent::AdapterContext;
use crate::errors::GardenerError;
use crate::runtime::ProcessRunner;
use crate::types::{AgentKind, RuntimeScope, WorkerState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeedTask {
    pub title: String,
    pub details: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SeedPayload {
    tasks: Vec<SeedTask>,
}

pub fn run_legacy_seed_runner_v1(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    backend: AgentKind,
    model: &str,
    prompt: &str,
) -> Result<Vec<SeedTask>, GardenerError> {
    let factory = AdapterFactory::with_defaults();
    let adapter = factory.get(backend).ok_or_else(|| {
        GardenerError::InvalidConfig(format!("adapter not registered for {:?}", backend))
    })?;

    let result = adapter.execute(
        process_runner,
        &AdapterContext {
            worker_id: "seed-worker".to_string(),
            session_id: "seed-session".to_string(),
            sandbox_id: "seed-sandbox".to_string(),
            model: model.to_string(),
            cwd: scope.working_dir.clone(),
            prompt_version: "seeding-v1".to_string(),
            context_manifest_hash: "seeding-context".to_string(),
            knowledge_refs: vec![],
            output_schema: None,
            output_file: Some(
                scope
                    .working_dir
                    .join(".cache/gardener/seed-last-message.json"),
            ),
            permissive_mode: true,
            max_turns: Some(12),
            cancel_requested: false,
        },
        prompt,
    )?;
    let _ = WorkerState::Seeding;

    let payload: SeedPayload = serde_json::from_value(result.payload)
        .map_err(|e| GardenerError::OutputEnvelope(e.to_string()))?;
    Ok(payload.tasks)
}

#[cfg(test)]
mod tests {
    use super::run_legacy_seed_runner_v1;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use crate::types::{AgentKind, RuntimeScope};
    use std::path::PathBuf;

    #[test]
    fn seed_runner_uses_codex_adapter_output_contract() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":{\"tasks\":[{\"title\":\"t\",\"details\":\"d\",\"rationale\":\"r\"}]}}\n".to_string(),
            stderr: String::new(),
        }));
        let tasks = run_legacy_seed_runner_v1(
            &runner,
            &RuntimeScope {
                process_cwd: PathBuf::from("/cwd"),
                repo_root: None,
                working_dir: PathBuf::from("/repo"),
            },
            AgentKind::Codex,
            "gpt-5-codex",
            "prompt",
        )
        .expect("tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "t");
    }
}
