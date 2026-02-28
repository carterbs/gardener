use gardener::agent::claude::ClaudeAdapter;
use gardener::agent::codex::CodexAdapter;
use gardener::agent::{AgentAdapter, AdapterContext};
use gardener::output_envelope::{parse_last_envelope, END_MARKER, START_MARKER};
use gardener::protocol::{map_codex_event, AgentEventKind, AgentTerminal};
use gardener::runtime::{FakeProcessRunner, ProcessOutput};
use gardener::triage_discovery::run_discovery;
use gardener::types::{AgentKind, RuntimeScope, WorkerState};
use serde_json::{json, Value};

fn claude_context() -> AdapterContext {
    AdapterContext {
        worker_id: "w".to_string(),
        session_id: "s".to_string(),
        sandbox_id: "x".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        cwd: "/repo".into(),
        prompt_version: "v1".to_string(),
        context_manifest_hash: "hash".to_string(),
        output_schema: None,
        output_file: None,
        permissive_mode: true,
        max_turns: Some(4),
    }
}

fn codex_context() -> AdapterContext {
    AdapterContext {
        worker_id: "w".to_string(),
        session_id: "s".to_string(),
        sandbox_id: "x".to_string(),
        model: "gpt-5-codex".to_string(),
        cwd: "/repo".into(),
        prompt_version: "v1".to_string(),
        context_manifest_hash: "hash".to_string(),
        output_schema: Some("/repo/schema.json".into()),
        output_file: Some("/repo/out.json".into()),
        permissive_mode: true,
        max_turns: None,
    }
}

#[test]
fn claude_result_without_subtype_is_failure() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "{\"type\":\"result\",\"result\":{\"summary\":\"done\"}}\n".to_string(),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(
            &runner,
            &claude_context(),
            "do a task",
            None,
        )
        .expect("result must parse");
    assert_ne!(result.terminal, AgentTerminal::Success);
    assert_eq!(result.terminal, AgentTerminal::Failure);
}

#[test]
fn claude_unknown_subtype_is_failure() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout:
            "{\"type\":\"result\",\"subtype\":\"partial\",\"result\":{\"summary\":\"partial\"}}\n"
                .to_string(),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Failure);
    assert_eq!(result.payload["summary"], "partial");
}

#[test]
fn claude_multiple_result_events_take_last_result() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout:
            "{\"type\":\"result\",\"subtype\":\"error\",\"result\":{\"summary\":\"first\"}}\n{\"type\":\"result\",\"subtype\":\"success\",\"result\":{\"summary\":\"last\"}}\n"
                .to_string(),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Success);
    assert_eq!(result.payload["summary"], "last");
}

#[test]
fn claude_empty_stdout_exit_zero_is_error() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let err = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect_err("terminal result required");
    assert!(format!("{err}").contains("missing terminal result event"));
}

#[test]
fn claude_nonzero_exit_without_result_event_is_process_error() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: "{\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\"}}\n{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"working\"}}\n".to_string(),
        stderr: "killed by OOM".to_string(),
    }));
    let adapter = ClaudeAdapter;
    let err = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect_err("terminal result required");
    assert!(format!("{err}").contains("killed by OOM"));
}

#[test]
fn claude_result_without_result_field_returns_null_payload() {
    // BUG: confirms current edge-case contract where malformed Claude success result payload is accepted as `Null`.
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "{\"type\":\"result\",\"subtype\":\"success\"}\n".to_string(),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Success);
    assert_eq!(result.payload, Value::Null);
}

#[test]
fn claude_stderr_does_not_affect_success() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "{\"type\":\"result\",\"subtype\":\"success\",\"result\":{\"summary\":\"ok\"}}\n"
            .to_string(),
        stderr: "WARNING: nearing token budget".to_string(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Success);
    assert!(result.diagnostics.iter().any(|line| line.contains("WARNING")));
}

#[test]
fn claude_non_json_stdout_is_ignored() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "NOTICE warmup\n{\"type\":\"result\",\"subtype\":\"success\",\"result\":{}}\n".to_string(),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Success);
    assert!(result
        .diagnostics
        .iter()
        .any(|line| line.contains("stdout non-json line ignored")));
}

#[test]
fn claude_multi_content_block_produces_multiple_turn_started_events() {
    // BUG B9: `content_block_start` is currently mapped to `TurnStarted` for each block.
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout:
            "{\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\"}}\n{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n{\"type\":\"content_block_stop\",\"index\":0}\n{\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n{\"type\":\"content_block_stop\",\"index\":1}\n{\"type\":\"result\",\"subtype\":\"success\",\"result\":{\"summary\":\"all good\"}}\n".to_string(),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Success);
    let turn_started = result
        .events
        .iter()
        .filter(|event| event.kind == AgentEventKind::TurnStarted)
        .count();
    assert_eq!(turn_started, 2);
}

#[test]
fn codex_both_failed_and_completed_prefers_first_failed_event() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout:
            "{\"type\":\"turn.failed\",\"reason\":\"first\"}\n{\"type\":\"turn.completed\",\"result\":{\"summary\":\"completed\"}}\n"
                .to_string(),
        stderr: String::new(),
    }));
    let adapter = CodexAdapter;
    let result = adapter
        .execute(&runner, &codex_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Failure);
}

#[test]
fn codex_completed_before_failed_still_reports_failure() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout:
            "{\"type\":\"turn.completed\",\"result\":{\"summary\":\"completed\"}}\n{\"type\":\"turn.failed\",\"reason\":\"late_failure\"}\n"
                .to_string(),
        stderr: String::new(),
    }));
    let adapter = CodexAdapter;
    let result = adapter
        .execute(&runner, &codex_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Failure);
}

#[test]
fn codex_error_event_is_treated_as_failure() {
    // BUG B8: Codex currently treats any "error" event as terminal failure.
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout:
            "{\"type\":\"turn.completed\",\"result\":{\"summary\":\"should be ignored\"}}\n{\"type\":\"error\",\"reason\":\"rate_limit_warning\",\"message\":\"approaching limit\"}\n{\"type\":\"turn.completed\",\"result\":{\"summary\":\"later success\"}}\n"
                .to_string(),
        stderr: String::new(),
    }));
    let adapter = CodexAdapter;
    let result = adapter
        .execute(&runner, &codex_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Failure);
    assert_eq!(result.payload["reason"], json!("rate_limit_warning"));
}

#[test]
fn codex_multiple_turn_completed_takes_last_event() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout:
            "{\"type\":\"turn.completed\",\"result\":{\"summary\":\"first\"}}\n{\"type\":\"turn.completed\",\"result\":{\"summary\":\"second\"}}\n"
                .to_string(),
        stderr: String::new(),
    }));
    let adapter = CodexAdapter;
    let result = adapter
        .execute(&runner, &codex_context(), "do a task", None)
        .expect("result must parse");
    assert_eq!(result.terminal, AgentTerminal::Success);
    assert_eq!(result.payload["summary"], "second");
}

#[test]
fn codex_empty_stdout_exit_zero_is_error() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }));
    let adapter = CodexAdapter;
    let err = adapter
        .execute(&runner, &codex_context(), "do a task", None)
        .expect_err("terminal event required");
    assert!(format!("{err}").contains("missing turn.completed or turn.failed"));
}

#[test]
fn discovery_envelope_with_invalid_json_fails_parse() {
    let scope = RuntimeScope {
        process_cwd: "/repo".into(),
        working_dir: "/repo".into(),
        repo_root: Some("/repo".into()),
    };
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: format!("{START_MARKER}\nnot valid json\n{END_MARKER}\n"),
        stderr: String::new(),
    }));
    let err = run_discovery(&runner, &scope, AgentKind::Codex, "gpt-5-codex", 12)
        .expect_err("invalid JSON should fail");
    assert!(format!("{err}").contains("invalid json"));
}

#[test]
fn discovery_envelope_missing_gardener_output_is_structurally_invalid() {
    let scope = RuntimeScope {
        process_cwd: "/repo".into(),
        working_dir: "/repo".into(),
        repo_root: Some("/repo".into()),
    };
    let payload = r#"{"schema_version":1,"state":"seeding","payload":{"wrong_key":"value"}}"#;
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: format!("{START_MARKER}\n{payload}\n{END_MARKER}\n"),
        stderr: String::new(),
    }));
    let err = run_discovery(&runner, &scope, AgentKind::Codex, "gpt-5-codex", 12)
        .expect_err("missing gardener_output should fail");
    assert!(format!("{err}").contains("gardener_output"));
}

#[test]
fn discovery_multiple_envelopes_uses_last_one() {
    let first = r#"{"schema_version":1,"state":"seeding","payload":{"note":"first"}}"#;
    let second = r#"{"schema_version":1,"state":"seeding","payload":{"note":"second"}}"#;
    let content = format!(
        "{START_MARKER}\n{first}\n{END_MARKER}\n{START_MARKER}\n{second}\n{END_MARKER}\n"
    );
    let envelope = parse_last_envelope(&content, WorkerState::Seeding)
        .expect("latest envelope should win");
    assert_eq!(envelope.payload["note"], "second");
}

#[test]
fn discovery_schema_version_2_rejected() {
    let payload = r#"{"schema_version":2,"state":"seeding","payload":{"agent_steering":{"grade":"unknown","summary":"n/a","issues":[],"strengths":[]},"knowledge_accessible":{"grade":"unknown","summary":"n/a","issues":[],"strengths":[]},"mechanical_guardrails":{"grade":"unknown","summary":"n/a","issues":[],"strengths":[]},"local_feedback_loop":{"grade":"unknown","summary":"n/a","issues":[],"strengths":[]},"coverage_signal":{"grade":"unknown","summary":"n/a","issues":[],"strengths":[]},"overall_readiness_score":10,"overall_readiness_grade":"F","primary_gap":"agent_steering","notable_findings":"","scope_notes":""}}"#;
    let stdout = format!(
        "{START_MARKER}\n{payload}\n{END_MARKER}\n"
    );
    let err = parse_last_envelope(&stdout, WorkerState::Seeding).expect_err("schema 2 should fail");
    assert!(format!("{err}").contains("schema_version must be 1"));
}

#[test]
fn discovery_end_marker_before_start_marker_is_rejected() {
    let payload = r#"{"schema_version":1,"state":"seeding","payload":{}}"#;
    let stdout = format!("{END_MARKER}\n{START_MARKER}\n{payload}\n");
    let err = parse_last_envelope(&stdout, WorkerState::Seeding)
        .expect_err("bad marker order should fail");
    assert!(format!("{err}").contains("before start"));
}

#[test]
fn discovery_payload_parse_shape_is_not_validated_by_parse_last_envelope() {
    let payload = r#"{"schema_version":1,"state":"seeding","payload":{"wrong_key":"value"}}"#;
    let stdout = format!(
        "{START_MARKER}\n{payload}\n{END_MARKER}\n"
    );
    let envelope = parse_last_envelope(&stdout, WorkerState::Seeding).expect("parse markers succeeds");
    assert_eq!(envelope.payload["wrong_key"], "value");
}

#[test]
fn codex_unknown_event_is_retained_in_event_stream() {
    let raw = map_codex_event(&json!({"type":"future.variant", "x": 1}));
    assert_eq!(raw.kind, AgentEventKind::Unknown);
    assert_eq!(raw.raw_type, "future.variant");
}
