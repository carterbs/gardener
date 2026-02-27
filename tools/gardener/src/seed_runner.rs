use crate::errors::GardenerError;
use crate::output_envelope::parse_last_envelope;
use crate::runtime::{ProcessRequest, ProcessRunner};
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
    let (program, args) = match backend {
        AgentKind::Codex => (
            "codex".to_string(),
            vec![
                "exec".to_string(),
                "--json".to_string(),
                "--model".to_string(),
                model.to_string(),
                prompt.to_string(),
            ],
        ),
        AgentKind::Claude => (
            "claude".to_string(),
            vec![
                "-p".to_string(),
                prompt.to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--model".to_string(),
                model.to_string(),
            ],
        ),
    };
    let output = process_runner.run(ProcessRequest {
        program,
        args,
        cwd: Some(scope.working_dir.clone()),
    })?;
    if output.exit_code != 0 {
        return Err(GardenerError::Process(output.stderr));
    }

    let envelope = parse_last_envelope(&output.stdout, WorkerState::Seeding)?;
    let payload: SeedPayload = serde_json::from_value(envelope.payload)
        .map_err(|e| GardenerError::OutputEnvelope(e.to_string()))?;
    Ok(payload.tasks)
}
