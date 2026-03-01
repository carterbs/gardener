use crate::agent::factory::AdapterFactory;
use crate::agent::AdapterContext;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::protocol::AgentEvent;
use crate::runtime::ProcessRunner;
use crate::types::{AgentKind, RuntimeScope};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeedTask {
    pub title: String,
    pub details: String,
    pub rationale: String,
    #[serde(default = "seed_domain_default")]
    pub domain: String,
    #[serde(default = "seed_priority_default")]
    pub priority: String,
}

fn seed_domain_default() -> String {
    "infrastructure".to_string()
}

fn seed_priority_default() -> String {
    "P1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SeedPayload {
    tasks: Vec<SeedTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SeedEnvelope {
    #[serde(default)]
    schema_version: Option<usize>,
    #[serde(default)]
    state: Option<String>,
    payload: SeedPayload,
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
            "prompt_version": "seeding-v2",
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
    let output_schema = seed_output_schema_path(scope)?;
    let context = AdapterContext {
        worker_id: "seed-worker".to_string(),
        session_id: "seed-session".to_string(),
        sandbox_id: "seed-sandbox".to_string(),
        model: model.to_string(),
        cwd: scope.working_dir.clone(),
        prompt_version: "seeding-v2".to_string(),
        context_manifest_hash: "seeding-context".to_string(),
        output_schema: Some(output_schema),
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
            "output_schema": context.output_schema.as_ref().map(|p| p.display().to_string()),
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

    let payload = parse_seed_payload(exec_result.payload).map_err(|e| {
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

fn parse_seed_payload(value: serde_json::Value) -> Result<SeedPayload, serde_json::Error> {
    if let Ok(payload) = serde_json::from_value::<SeedPayload>(value.clone()) {
        return Ok(payload);
    }
    let envelope: SeedEnvelope = serde_json::from_value(value)?;
    Ok(envelope.payload)
}

fn seed_output_schema_path(scope: &RuntimeScope) -> Result<PathBuf, GardenerError> {
    append_run_log(
        "debug",
        "seed_runner.schema_path",
        json!({
            "working_dir": scope.working_dir.display().to_string(),
        }),
    );
    let path = scope
        .working_dir
        .join(".cache/gardener/schemas/seed_task_schema.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| GardenerError::Io(format!("create_dir_all {}: {e}", parent.display())))?;
    }

    let desired = seed_output_schema();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing != desired {
        std::fs::write(&path, desired)
            .map_err(|e| GardenerError::Io(format!("write schema {}: {e}", path.display())))?;
    }
    Ok(path)
}

fn seed_output_schema() -> String {
    r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "schema_version": {
      "type": "integer",
      "const": 1
    },
    "state": {
      "type": "string",
      "const": "seeding"
    },
    "payload": {
      "type": "object",
      "required": ["tasks"],
      "properties": {
        "tasks": {
          "type": "array",
          "minItems": 1,
          "maxItems": 12,
          "items": {
            "type": "object",
            "required": ["title", "details", "rationale", "domain", "priority"],
            "properties": {
              "title": { "type": "string", "minLength": 5 },
              "details": { "type": "string", "minLength": 5 },
              "rationale": { "type": "string", "minLength": 10 },
              "domain": { "type": "string", "minLength": 1 },
              "priority": { "type": "string", "enum": ["P0", "P1", "P2"] }
            }
          }
        }
      }
    }
  },
  "required": ["schema_version", "state", "payload"]
}"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::run_legacy_seed_runner_v1;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use crate::types::{AgentKind, RuntimeScope};
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn seed_runner_uses_codex_adapter_output_contract() {
        let runner = FakeProcessRunner::default();
        let working_dir = tempdir().expect("tempdir");
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":{\"schema_version\":1,\"state\":\"seeding\",\"payload\":{\"tasks\":[{\"title\":\"t\",\"details\":\"d\",\"rationale\":\"rationale\", \"domain\":\"backlog\",\"priority\":\"P1\"}]}}}\n".to_string(),
            stderr: String::new(),
        }));
        let tasks = run_legacy_seed_runner_v1(
            &runner,
            &RuntimeScope {
                process_cwd: PathBuf::from("/cwd"),
                repo_root: None,
                working_dir: working_dir.path().to_path_buf(),
            },
            AgentKind::Codex,
            "gpt-5-codex",
            "prompt",
        )
        .expect("tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "t");
        assert_eq!(tasks[0].domain, "backlog");
        assert_eq!(tasks[0].priority, "P1");
    }
}
