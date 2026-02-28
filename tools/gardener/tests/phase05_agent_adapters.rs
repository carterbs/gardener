use gardener::agent::claude::ClaudeAdapter;
use gardener::agent::codex::CodexAdapter;
use gardener::agent::{AdapterContext, AgentAdapter};
use gardener::output_envelope::parse_last_envelope;
use gardener::protocol::{AgentEventKind, AgentTerminal};
use gardener::runtime::{FakeProcessRunner, ProcessOutput};
use gardener::triage_discovery::{run_discovery, DiscoveryAssessment};
use gardener::types::{AgentKind, RuntimeScope, WorkerState};
use std::path::{Path, PathBuf};

fn load_fixture(relative: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent-responses")
            .join(relative),
    )
    .expect("test fixture should not fail")
}

fn claude_context() -> AdapterContext {
    AdapterContext {
        worker_id: "w".to_string(),
        session_id: "s".to_string(),
        sandbox_id: "x".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        cwd: PathBuf::from("/repo"),
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
        cwd: PathBuf::from("/repo"),
        prompt_version: "v1".to_string(),
        context_manifest_hash: "hash".to_string(),
        output_schema: Some(PathBuf::from("/repo/schema.json")),
        output_file: Some(PathBuf::from("/repo/out.json")),
        permissive_mode: true,
        max_turns: None,
    }
}

// --- Claude Adapter Tests ---

#[test]
fn claude_happy_path_returns_success() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: load_fixture("claude/happy-path.jsonl"),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect("should succeed");
    assert_eq!(result.terminal, AgentTerminal::Success);
    assert!(
        result.events.iter().any(|e| e.kind == AgentEventKind::ToolCall),
        "should contain a ToolCall event"
    );
    assert_eq!(runner.spawned()[0].program, "claude");
    assert!(runner.spawned()[0].args.contains(&"-p".to_string()));
}

#[test]
fn claude_turn_failed_returns_failure() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: load_fixture("claude/turn-failed.jsonl"),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect("should return result, not Err");
    assert_eq!(result.terminal, AgentTerminal::Failure);
}

#[test]
fn claude_malformed_lines_are_skipped_not_fatal() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: load_fixture("claude/malformed-ndjson.jsonl"),
        stderr: String::new(),
    }));
    let adapter = ClaudeAdapter;
    let result = adapter
        .execute(&runner, &claude_context(), "do a task", None)
        .expect("should succeed despite malformed lines");
    assert_eq!(result.terminal, AgentTerminal::Success);
    assert!(
        result.diagnostics.iter().any(|d| d.contains("non-json") || d.contains("ignored")),
        "diagnostics should mention skipped lines: {:?}",
        result.diagnostics
    );
}

// --- Codex Adapter Tests ---

#[test]
fn codex_happy_path_returns_success() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: load_fixture("codex/happy-path.jsonl"),
        stderr: String::new(),
    }));
    let adapter = CodexAdapter;
    let result = adapter
        .execute(&runner, &codex_context(), "do a task", None)
        .expect("should succeed");
    assert_eq!(result.terminal, AgentTerminal::Success);
    assert!(
        result.events.iter().any(|e| e.kind == AgentEventKind::ToolCall),
        "should contain ToolCall events from item.started/item.updated"
    );
    assert_eq!(runner.spawned()[0].program, "codex");
}

#[test]
fn codex_turn_failed_detected_before_completed_scan() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: load_fixture("codex/turn-failed.jsonl"),
        stderr: String::new(),
    }));
    let adapter = CodexAdapter;
    let result = adapter
        .execute(&runner, &codex_context(), "do a task", None)
        .expect("should return result, not Err");
    assert_eq!(result.terminal, AgentTerminal::Failure);
}

#[test]
fn codex_error_event_returns_failure() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: load_fixture("codex/error-event.jsonl"),
        stderr: String::new(),
    }));
    let adapter = CodexAdapter;
    let result = adapter
        .execute(&runner, &codex_context(), "do a task", None)
        .expect("should return result, not Err");
    assert_eq!(result.terminal, AgentTerminal::Failure);
}

// --- Discovery / Envelope Tests ---

#[test]
fn discovery_valid_envelope_parsed() {
    let content = load_fixture("discovery/codex-discovery.jsonl");
    let envelope = parse_last_envelope(&content, WorkerState::Seeding).expect("should parse");
    assert_eq!(envelope.schema_version, 1);
    assert_eq!(envelope.state, WorkerState::Seeding);
    let payload = envelope.payload;
    let output = &payload["gardener_output"];
    assert_eq!(output["overall_readiness_grade"], "C");
    assert_eq!(output["primary_gap"], "mechanical_guardrails");
    assert_eq!(output["overall_readiness_score"], 68);
}

#[test]
fn discovery_no_envelope_returns_error() {
    let content = load_fixture("discovery/no-envelope.jsonl");
    let err = parse_last_envelope(&content, WorkerState::Seeding).expect_err("should fail");
    assert!(
        format!("{err}").contains("missing start marker"),
        "error should mention missing marker: {err}"
    );
}

#[test]
fn discovery_wrong_state_returns_error() {
    let content = load_fixture("discovery/wrong-state.jsonl");
    let err = parse_last_envelope(&content, WorkerState::Seeding).expect_err("should fail");
    assert!(
        format!("{err}").contains("state mismatch"),
        "error should mention state mismatch: {err}"
    );
}

#[test]
fn discovery_nonzero_exit_code_returns_error() {
    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: "agent crashed".to_string(),
    }));
    let scope = RuntimeScope {
        process_cwd: PathBuf::from("/repo"),
        working_dir: PathBuf::from("/repo"),
        repo_root: Some(PathBuf::from("/repo")),
    };
    let err = run_discovery(&runner, &scope, AgentKind::Codex, "gpt-5-codex", 4)
        .expect_err("should fail on non-zero exit");
    assert!(
        format!("{err}").contains("agent crashed"),
        "error should contain stderr: {err}"
    );
}

#[test]
fn discovery_assessment_unknown_has_f_grade() {
    let unknown = DiscoveryAssessment::unknown();
    assert_eq!(unknown.overall_readiness_grade, "F");
    assert_eq!(unknown.primary_gap, "agent_steering");
    assert_eq!(unknown.overall_readiness_score, 10);
}
