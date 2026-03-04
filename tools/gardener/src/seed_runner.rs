use crate::agent::factory::AdapterFactory;
use crate::agent::AdapterContext;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::prompt_registry::{SEEDING_PROMPT_VERSION_DIRECT, SEEDING_PROMPT_VERSION_LEGACY};
use crate::protocol::{AgentEvent, AgentTerminal};
use crate::runtime::ProcessRunner;
use crate::types::{AgentKind, RuntimeScope};
use serde::de::Error as DeError;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
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
    schema_version: usize,
    state: String,
    payload: SeedPayload,
}

pub fn run_legacy_seed_runner_v1(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    backend: AgentKind,
    model: &str,
    prompt: &str,
) -> Result<Vec<SeedTask>, GardenerError> {
    run_legacy_seed_runner_v1_with_events_internal(
        process_runner,
        scope,
        backend,
        model,
        prompt,
        None,
        None,
    )
}

pub fn run_legacy_seed_runner_v1_with_events(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    backend: AgentKind,
    model: &str,
    prompt: &str,
    on_event: Option<&mut dyn FnMut(&AgentEvent)>,
) -> Result<Vec<SeedTask>, GardenerError> {
    run_legacy_seed_runner_v1_with_events_internal(
        process_runner,
        scope,
        backend,
        model,
        prompt,
        on_event,
        None,
    )
}

pub fn run_legacy_seed_runner_v1_with_events_and_task_count(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    backend: AgentKind,
    model: &str,
    prompt: &str,
    expected_task_count: usize,
    on_event: Option<&mut dyn FnMut(&AgentEvent)>,
) -> Result<Vec<SeedTask>, GardenerError> {
    run_legacy_seed_runner_v1_with_events_internal(
        process_runner,
        scope,
        backend,
        model,
        prompt,
        on_event,
        Some(expected_task_count),
    )
}

fn run_legacy_seed_runner_v1_with_events_internal(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    backend: AgentKind,
    model: &str,
    prompt: &str,
    mut on_event: Option<&mut dyn FnMut(&AgentEvent)>,
    expected_task_count: Option<usize>,
) -> Result<Vec<SeedTask>, GardenerError> {
    append_run_log(
        "info",
        "seed_runner.started",
        json!({
            "backend": format!("{:?}", backend),
            "model": model,
            "working_dir": scope.working_dir.display().to_string(),
            "prompt_version": SEEDING_PROMPT_VERSION_LEGACY,
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

    let output_schema = seed_output_schema_path(scope)?;
    let context = AdapterContext {
        worker_id: "seed-worker".to_string(),
        session_id: "seed-session".to_string(),
        sandbox_id: "seed-sandbox".to_string(),
        model: model.to_string(),
        cwd: scope.working_dir.clone(),
        prompt_version: SEEDING_PROMPT_VERSION_LEGACY.to_string(),
        context_manifest_hash: "seeding-context".to_string(),
        output_schema: Some(output_schema),
        output_file: None,
        permissive_mode: true,
        max_turns: Some(12),
    };

    append_run_log(
        "debug",
            "seed_runner.adapter.executing",
        json!({
            "backend": format!("{:?}", backend),
            "model": model,
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

    if exec_result.terminal == AgentTerminal::Failure {
        let reason = if exec_result.payload.is_null() {
            "agent reported failure".to_string()
        } else {
            exec_result.payload.to_string()
        };
        append_run_log(
            "error",
            "seed_runner.turn_failed",
            json!({
                "backend": format!("{:?}", backend),
                "model": model,
                "payload": exec_result.payload,
                "diagnostics": exec_result.diagnostics,
            }),
        );
        return Err(GardenerError::Process(format!(
            "seed turn failed: {reason}"
        )));
    }

    let payload_result = match expected_task_count {
        Some(expected_task_count) => {
            parse_seed_payload_with_task_count(exec_result.payload, Some(expected_task_count))
        }
        None => parse_seed_payload(exec_result.payload),
    };
    let payload = payload_result.map_err(|err| {
        GardenerError::OutputEnvelope(format!(
            "seed output must match seeding envelope schema: {err}"
        ))
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

pub fn run_seed_agent_direct_v2_with_events(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    backend: AgentKind,
    model: &str,
    prompt: &str,
    mut on_event: Option<&mut dyn FnMut(&AgentEvent)>,
) -> Result<(), GardenerError> {
    append_run_log(
        "info",
        "seed_runner.direct.started",
        json!({
            "backend": format!("{:?}", backend),
            "model": model,
            "working_dir": scope.working_dir.display().to_string(),
            "prompt_version": SEEDING_PROMPT_VERSION_DIRECT,
            "max_turns": 24,
        }),
    );

    let factory = AdapterFactory::with_defaults();
    let adapter = factory.get(backend).ok_or_else(|| {
        let err = format!("adapter not registered for {:?}", backend);
        append_run_log(
            "error",
            "seed_runner.direct.adapter_not_found",
            json!({ "backend": format!("{:?}", backend), "error": err }),
        );
        GardenerError::InvalidConfig(err)
    })?;

    let context = AdapterContext {
        worker_id: "seed-worker".to_string(),
        session_id: "seed-session".to_string(),
        sandbox_id: "seed-sandbox".to_string(),
        model: model.to_string(),
        cwd: scope.working_dir.clone(),
        prompt_version: SEEDING_PROMPT_VERSION_DIRECT.to_string(),
        context_manifest_hash: "seeding-context-direct".to_string(),
        output_schema: None,
        output_file: None,
        permissive_mode: true,
        max_turns: Some(24),
    };

    let result = if let Some(sink) = on_event.as_mut() {
        adapter.execute(process_runner, &context, prompt, Some(*sink))
    } else {
        adapter.execute(process_runner, &context, prompt, None)
    }?;

    match result.terminal {
        AgentTerminal::Success => {
            append_run_log(
                "info",
                "seed_runner.direct.completed",
                json!({
                    "backend": format!("{:?}", backend),
                    "model": model,
                }),
            );
            Ok(())
        }
        AgentTerminal::Failure => {
            let reason = if result.payload.is_null() {
                "agent reported failure".to_string()
            } else {
                result.payload.to_string()
            };
            append_run_log(
                "error",
                "seed_runner.direct.failed",
                json!({
                    "backend": format!("{:?}", backend),
                    "model": model,
                    "payload": result.payload,
                    "diagnostics": result.diagnostics,
                }),
            );
            Err(GardenerError::Process(format!(
                "direct seed runner failed: {reason}"
            )))
        }
    }
}

fn parse_seed_payload(value: serde_json::Value) -> Result<SeedPayload, serde_json::Error> {
    parse_seed_payload_with_task_count(value, None)
}

fn parse_seed_payload_with_task_count(
    value: serde_json::Value,
    expected_task_count: Option<usize>,
) -> Result<SeedPayload, serde_json::Error> {
    let envelope: SeedEnvelope = serde_json::from_value(value).map_err(|err| {
        DeError::custom(format!("seed payload must be a seeding envelope: {err}"))
    })?;
    if envelope.schema_version != 1 {
        return Err(DeError::custom(format!(
            "schema_version must be 1, found {}",
            envelope.schema_version
        )));
    }
    if envelope.state != "seeding" {
        return Err(DeError::custom(format!(
            "state mismatch: expected seeding, got {}",
            envelope.state
        )));
    }
    validate_seed_payload(&envelope.payload, expected_task_count)?;
    Ok(envelope.payload)
}

fn validate_seed_payload(
    payload: &SeedPayload,
    expected_task_count: Option<usize>,
) -> Result<(), serde_json::Error> {
    if let Some(expected_task_count) = expected_task_count {
        if payload.tasks.len() != expected_task_count {
            return Err(DeError::custom(format!(
                "expected {expected_task_count} tasks in dry-run payload, found {}",
                payload.tasks.len()
            )));
        }
    }

    let mut titles = HashSet::new();
    for task in &payload.tasks {
        let normalized_title = task.title.trim().to_ascii_lowercase();
        if !titles.insert(normalized_title) {
            return Err(DeError::custom(format!(
                "duplicate task title in payload: {}",
                task.title
            )));
        }

        if is_placeholder_domain(&task.domain) {
            return Err(DeError::custom(format!(
                "invalid placeholder domain in task {}: {}",
                task.title,
                task.domain
            )));
        }
    }

    Ok(())
}

fn is_placeholder_domain(domain: &str) -> bool {
    let normalized = domain.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }

    matches!(
        normalized.as_str(),
        "placeholder" | "todo" | "tbd" | "n/a" | "na" | "none" | "unknown" | "to be decided" | "to be determined" | "not set" | "unassigned"
    ) || normalized.contains("placeholder")
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
  "additionalProperties": false,
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
      "additionalProperties": false,
      "required": ["tasks"],
      "properties": {
        "tasks": {
          "type": "array",
            "minItems": 10,
            "maxItems": 10,
          "items": {
            "type": "object",
            "additionalProperties": false,
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
    use super::{
        parse_seed_payload, run_legacy_seed_runner_v1,
        run_legacy_seed_runner_v1_with_events, run_legacy_seed_runner_v1_with_events_and_task_count,
        parse_seed_payload_with_task_count, run_seed_agent_direct_v2_with_events, SeedTask,
    };
    use crate::errors::GardenerError;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use crate::types::{AgentKind, RuntimeScope};
    use std::path::PathBuf;
    use tempfile::tempdir;

    const DRY_RUN_TASK_COUNT: usize = 10;

    fn codex_scope(working_dir: &std::path::Path) -> RuntimeScope {
        RuntimeScope {
            process_cwd: PathBuf::from("/cwd"),
            repo_root: None,
            working_dir: working_dir.to_path_buf(),
        }
    }

    #[test]
    fn direct_v2_returns_ok_on_codex_success() {
        let runner = FakeProcessRunner::default();
        let dir = tempdir().expect("tempdir");
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":{\"summary\":\"seeded 10 tasks\"}}\n"
                .to_string(),
            stderr: String::new(),
        }));
        let result = run_seed_agent_direct_v2_with_events(
            &runner,
            &codex_scope(dir.path()),
            AgentKind::Codex,
            "gpt-5-codex",
            "seed the backlog",
            None,
        );
        assert!(result.is_ok(), "expected Ok but got {result:?}");
    }

    #[test]
    fn direct_v2_returns_err_on_codex_failure() {
        let runner = FakeProcessRunner::default();
        let dir = tempdir().expect("tempdir");
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.failed\",\"reason\":\"agent could not seed\"}\n".to_string(),
            stderr: String::new(),
        }));
        let result = run_seed_agent_direct_v2_with_events(
            &runner,
            &codex_scope(dir.path()),
            AgentKind::Codex,
            "gpt-5-codex",
            "seed the backlog",
            None,
        );
        assert!(result.is_err());
        let msg = result.expect_err("expected Err").to_string();
        assert!(
            msg.contains("direct seed runner failed"),
            "unexpected message: {msg}"
        );
    }

    #[test]
    fn direct_v2_failure_with_null_payload_uses_fallback_message() {
        let runner = FakeProcessRunner::default();
        let dir = tempdir().expect("tempdir");
        // turn.failed with no result field → payload is null
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.failed\"}\n".to_string(),
            stderr: String::new(),
        }));
        let result = run_seed_agent_direct_v2_with_events(
            &runner,
            &codex_scope(dir.path()),
            AgentKind::Codex,
            "gpt-5-codex",
            "seed the backlog",
            None,
        );
        assert!(result.is_err());
        let msg = result.expect_err("expected Err").to_string();
        assert!(
            msg.contains("agent reported failure") || msg.contains("direct seed runner failed"),
            "unexpected message: {msg}"
        );
    }

    #[test]
    fn direct_v2_calls_on_event_callback() {
        use crate::protocol::AgentEvent;
        let runner = FakeProcessRunner::default();
        let dir = tempdir().expect("tempdir");
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":{\"summary\":\"done\"}}\n".to_string(),
            stderr: String::new(),
        }));
        let mut event_count = 0usize;
        let mut on_event = |_event: &AgentEvent| {
            event_count += 1;
        };
        let result = run_seed_agent_direct_v2_with_events(
            &runner,
            &codex_scope(dir.path()),
            AgentKind::Codex,
            "gpt-5-codex",
            "seed the backlog",
            Some(&mut on_event),
        );
        assert!(result.is_ok());
        assert!(
            event_count > 0,
            "expected on_event to be called at least once"
        );
    }

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

    #[test]
    fn legacy_v1_rejects_unwrapped_direct_payload() {
        let runner = FakeProcessRunner::default();
        let working_dir = tempdir().expect("tempdir");
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":{\"title\":\"t\",\"details\":\"d\",\"rationale\":\"rationale\",\"domain\":\"backlog\",\"priority\":\"P1\"}}\n".to_string(),
            stderr: String::new(),
        }));
        let result = run_legacy_seed_runner_v1(
            &runner,
            &RuntimeScope {
                process_cwd: PathBuf::from("/cwd"),
                repo_root: None,
                working_dir: working_dir.path().to_path_buf(),
            },
            AgentKind::Codex,
            "gpt-5-codex",
            "prompt",
        );
        assert!(result.is_err(), "expected Err on unwrapped payload");
        let msg = result.expect_err("expected parse error");
        assert!(
            msg.to_string().contains("seed output must match seeding envelope schema"),
            "unexpected message: {msg}"
        );
    }

    #[test]
    fn legacy_v1_on_event_callback_is_invoked() {
        use crate::protocol::AgentEvent;
        let runner = FakeProcessRunner::default();
        let working_dir = tempdir().expect("tempdir");
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":{\"schema_version\":1,\"state\":\"seeding\",\"payload\":{\"tasks\":[{\"title\":\"t\",\"details\":\"d\",\"rationale\":\"r\",\"domain\":\"backlog\",\"priority\":\"P1\"}]}}}\n".to_string(),
            stderr: String::new(),
        }));
        let mut event_count = 0usize;
        let mut on_event = |_event: &AgentEvent| {
            event_count += 1;
        };
        let tasks = run_legacy_seed_runner_v1_with_events(
            &runner,
            &RuntimeScope {
                process_cwd: PathBuf::from("/cwd"),
                repo_root: None,
                working_dir: working_dir.path().to_path_buf(),
            },
            AgentKind::Codex,
            "gpt-5-codex",
            "prompt",
            Some(&mut on_event),
        )
        .expect("tasks");
        assert_eq!(tasks.len(), 1);
        assert!(
            event_count > 0,
            "on_event should have been called at least once"
        );
    }

    #[test]
    fn legacy_v1_returns_err_on_exec_failure() {
        let runner = FakeProcessRunner::default();
        let working_dir = tempdir().expect("tempdir");
        runner.push_response(Err(GardenerError::Process(
            "agent process died".to_string(),
        )));
        let result = run_legacy_seed_runner_v1(
            &runner,
            &RuntimeScope {
                process_cwd: PathBuf::from("/cwd"),
                repo_root: None,
                working_dir: working_dir.path().to_path_buf(),
            },
            AgentKind::Codex,
            "gpt-5-codex",
            "prompt",
        );
        assert!(result.is_err(), "expected Err on exec failure");
    }

    #[test]
    fn legacy_v1_returns_err_on_bad_payload() {
        let runner = FakeProcessRunner::default();
        let working_dir = tempdir().expect("tempdir");
        // result is a string, not a SeedPayload or SeedEnvelope
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":\"not-an-object\"}\n".to_string(),
            stderr: String::new(),
        }));
        let result = run_legacy_seed_runner_v1(
            &runner,
            &RuntimeScope {
                process_cwd: PathBuf::from("/cwd"),
                repo_root: None,
                working_dir: working_dir.path().to_path_buf(),
            },
            AgentKind::Codex,
            "gpt-5-codex",
            "prompt",
        );
        assert!(result.is_err(), "expected Err on unparseable payload");
    }

    #[test]
    fn legacy_v1_with_task_count_rejects_wrong_payload_size() {
        let runner = FakeProcessRunner::default();
        let working_dir = tempdir().expect("tempdir");
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":{\"schema_version\":1,\"state\":\"seeding\",\"payload\":{\"tasks\":[{\"title\":\"t\",\"details\":\"d\",\"rationale\":\"r\",\"domain\":\"backlog\",\"priority\":\"P1\"}]}}}\n".to_string(),
            stderr: String::new(),
        }));
        let result = run_legacy_seed_runner_v1_with_events_and_task_count(
            &runner,
            &RuntimeScope {
                process_cwd: PathBuf::from("/cwd"),
                repo_root: None,
                working_dir: working_dir.path().to_path_buf(),
            },
            AgentKind::Codex,
            "gpt-5-codex",
            "prompt",
            DRY_RUN_TASK_COUNT,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn seed_task_serde_defaults_apply_when_fields_absent() {
        let json =
            r#"{"title":"test task","details":"some details","rationale":"some rationale here"}"#;
        let task: SeedTask = serde_json::from_str(json).expect("parse SeedTask");
        assert_eq!(task.domain, "infrastructure");
        assert_eq!(task.priority, "P1");
    }

    #[test]
    fn parse_seed_payload_rejects_direct_payload_format() {
        let payload = serde_json::json!({
            "tasks": [{
                "title": "t",
                "details": "d",
                "rationale": "r",
                "domain": "backlog",
                "priority": "P1"
            }]
        });
        let result = parse_seed_payload(payload);
        assert!(result.is_err(), "expected Err for direct payload");
    }

    #[test]
    fn parse_seed_payload_rejects_bad_envelope_metadata() {
        let payload = serde_json::json!({
            "schema_version": 2,
            "state": "wrong",
            "payload": {
                "tasks": [{
                    "title": "t",
                    "details": "d",
                    "rationale": "rationale",
                    "domain": "backlog",
                    "priority": "P1"
                }]
            }
        });
        let result = parse_seed_payload(payload);
        let err = result.expect_err("expected invalid metadata");
        assert!(
            err.to_string().contains("schema_version must be 1")
                || err.to_string().contains("state mismatch"),
            "unexpected parse error: {err}"
        );
    }

    #[test]
    fn parse_seed_payload_accepts_envelope_payload_format() {
        let payload = serde_json::json!({
            "schema_version": 1,
            "state": "seeding",
            "payload": {
                "tasks": [{
                    "title": "t",
                    "details": "d",
                    "rationale": "r",
                    "domain": "backlog",
                    "priority": "P1"
                }]
            }
        });
        let result = parse_seed_payload(payload);
        assert!(result.is_ok());
        assert_eq!(result.expect("parse_seed_payload succeeded").tasks.len(), 1);
    }

    #[test]
    fn parse_seed_payload_with_task_count_enforces_exact_task_count() {
        let payload = serde_json::json!({
            "schema_version": 1,
            "state": "seeding",
            "payload": {
                "tasks": [{
                    "title": "a",
                    "details": "d",
                    "rationale": "rationale",
                    "domain": "backlog",
                    "priority": "P1"
                }]
            }
        });
        let result = parse_seed_payload_with_task_count(payload, Some(DRY_RUN_TASK_COUNT));
        assert!(
            result.is_err(),
            "expected Err for dry-run payload task count mismatch"
        );
    }

    #[test]
    fn parse_seed_payload_with_task_count_rejects_duplicate_titles() {
        let tasks: Vec<serde_json::Value> = (0..10)
            .map(|index| {
        serde_json::json!({
                    "title": if index % 2 == 0 {
                        "same title".to_string()
                    } else {
                        format!("t{index}")
                    },
                    "details": "details here",
                    "rationale": "rationale is detailed",
                    "domain": "backlog",
                    "priority": "P1"
                })
            })
            .collect();
        let payload = serde_json::json!({
            "schema_version": 1,
            "state": "seeding",
            "payload": {
                "tasks": tasks
            }
        });
        let result = parse_seed_payload_with_task_count(payload, Some(DRY_RUN_TASK_COUNT));
        assert!(
            result.is_err(),
            "expected Err for duplicate titles in dry-run payload"
        );
    }

    #[test]
    fn parse_seed_payload_with_task_count_rejects_placeholder_domain() {
        let tasks: Vec<serde_json::Value> = (0..10)
            .map(|index| {
                serde_json::json!({
                    "title": format!("task {index}"),
                    "details": "details here",
                    "rationale": "rationale is detailed",
                    "domain": if index == 0 { "todo" } else { "backlog" },
                    "priority": "P1"
                })
            })
            .collect();
        let payload = serde_json::json!({
            "schema_version": 1,
            "state": "seeding",
            "payload": {
                "tasks": tasks
            }
        });
        let result = parse_seed_payload_with_task_count(payload, Some(DRY_RUN_TASK_COUNT));
        assert!(
            result.is_err(),
            "expected Err for placeholder task domain in dry-run payload"
        );
    }

    #[test]
    fn parse_seed_payload_returns_err_on_invalid_value() {
        let payload = serde_json::json!("not-an-object");
        let result = parse_seed_payload(payload);
        assert!(result.is_err());
    }

    #[test]
    fn seed_output_schema_is_strict() {
        let schema: serde_json::Value =
            serde_json::from_str(&super::seed_output_schema()).expect("valid JSON schema");
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
        assert_eq!(
            schema["properties"]["payload"]["additionalProperties"],
            serde_json::json!(false)
        );
        assert_eq!(
            schema["properties"]["payload"]["properties"]["tasks"]["items"]["additionalProperties"],
            serde_json::json!(false)
        );
        assert_eq!(
            schema["properties"]["payload"]["properties"]["tasks"]["minItems"],
            serde_json::json!(10)
        );
        assert_eq!(
            schema["properties"]["payload"]["properties"]["tasks"]["maxItems"],
            serde_json::json!(10)
        );
    }
}
