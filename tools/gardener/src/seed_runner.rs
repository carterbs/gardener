use crate::agent::factory::AdapterFactory;
use crate::agent::AdapterContext;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::protocol::AgentEvent;
use crate::runtime::ProcessRunner;
use crate::types::{AgentKind, RuntimeScope};
use serde::{Deserialize, Serialize};
use serde_json::json;

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
    run_legacy_seed_runner_v1_with_events(process_runner, scope, backend, model, prompt, None)
}

pub fn run_legacy_seed_runner_v1_with_events(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    backend: AgentKind,
    model: &str,
    prompt: &str,
    mut on_event: Option<&mut dyn FnMut(&AgentEvent)>,
) -> Result<Vec<SeedTask>, GardenerError> {
    append_run_log(
        "info",
        "seed_runner.started",
        json!({
            "backend": format!("{:?}", backend),
            "model": model,
            "working_dir": scope.working_dir.display().to_string(),
            "prompt_version": "seeding-v1",
            "max_turns": 12,
        }),
    );

    let factory = AdapterFactory::with_defaults();
    let adapter = factory.get(backend).ok_or_else(|| {
        let err = format!("adapter not registered for {:?}", backend);
        append_run_log(
            "error",
            "seed_runner.adapter_not_found",
            json!({ "backend": format!("{:?}", backend), "error": err }),
        );
        GardenerError::InvalidConfig(err)
    })?;

    let output_file = scope
        .working_dir
        .join(".cache/gardener/seed-last-message.json");
    let context = AdapterContext {
        worker_id: "seed-worker".to_string(),
        session_id: "seed-session".to_string(),
        sandbox_id: "seed-sandbox".to_string(),
        model: model.to_string(),
        cwd: scope.working_dir.clone(),
        prompt_version: "seeding-v1".to_string(),
        context_manifest_hash: "seeding-context".to_string(),
        output_schema: None,
        output_file: Some(output_file.clone()),
        permissive_mode: true,
        max_turns: Some(12),
    };

    append_run_log(
        "debug",
        "seed_runner.adapter.executing",
        json!({
            "backend": format!("{:?}", backend),
            "model": model,
            "output_file": output_file.display().to_string(),
        }),
    );

    let result = if let Some(sink) = on_event.as_mut() {
        adapter.execute(process_runner, &context, prompt, Some(*sink))
    } else {
        adapter.execute(process_runner, &context, prompt, None)
    };

    let exec_result = match result {
        Ok(r) => r,
        Err(e) => {
            append_run_log(
                "error",
                "seed_runner.adapter.failed",
                json!({
                    "backend": format!("{:?}", backend),
                    "model": model,
                    "error": e.to_string(),
                }),
            );
            return Err(e);
        }
    };

    let payload: SeedPayload = serde_json::from_value(exec_result.payload).map_err(|e| {
        append_run_log(
            "error",
            "seed_runner.parse_failed",
            json!({ "error": e.to_string() }),
        );
        GardenerError::OutputEnvelope(e.to_string())
    })?;

    append_run_log(
        "info",
        "seed_runner.completed",
        json!({
            "backend": format!("{:?}", backend),
            "model": model,
            "task_count": payload.tasks.len(),
        }),
    );

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
