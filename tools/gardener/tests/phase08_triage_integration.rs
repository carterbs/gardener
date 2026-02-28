use gardener::config::AppConfig;
use gardener::output_envelope::parse_last_envelope;
use gardener::runtime::{
    FakeClock, FakeFileSystem, FakeProcessRunner, FakeTerminal, ProcessOutput,
    ProductionRuntime,
};
use gardener::triage::{triage_needed, TriageDecision};
use gardener::triage_agent_detection::{is_non_interactive, EnvMap};
use gardener::triage_discovery::DiscoveryAssessment;
use gardener::types::{RuntimeScope, WorkerState};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn load_fixture(relative: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent-responses")
            .join(relative),
    )
    .expect("test fixture should not fail")
}

fn default_scope() -> RuntimeScope {
    RuntimeScope {
        process_cwd: PathBuf::from("/repo"),
        working_dir: PathBuf::from("/repo"),
        repo_root: Some(PathBuf::from("/repo")),
    }
}

fn default_config() -> AppConfig {
    AppConfig::default()
}

fn default_profile_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
        .join(".gardener")
        .join("repo")
        .join("repo-intelligence.toml")
}

// --- Triage Decision Tests ---

#[test]
fn triage_needed_when_profile_missing() {
    let fs = FakeFileSystem::default();
    let process = FakeProcessRunner::default();
    let runtime = ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process),
        terminal: Arc::new(FakeTerminal::new(true)),
    };
    let scope = default_scope();
    let cfg = default_config();
    let decision = triage_needed(&scope, &cfg, &runtime, false).expect("should not error");
    assert_eq!(decision, TriageDecision::Needed);
}

#[test]
fn triage_needed_when_force_retriage() {
    let profile_path = default_profile_path();
    let profile_toml = include_str!("fixtures/triage/expected-profiles/phase03-profile.toml");
    let fs = FakeFileSystem::with_file(profile_path, profile_toml);
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "matching_sha\n".to_string(),
        stderr: String::new(),
    }));
    let runtime = ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process),
        terminal: Arc::new(FakeTerminal::new(true)),
    };
    let scope = default_scope();
    let cfg = default_config();
    let decision = triage_needed(&scope, &cfg, &runtime, true).expect("should not error");
    assert_eq!(decision, TriageDecision::Needed);
}

// --- Discovery / Envelope Parsing Tests ---

#[test]
fn discovery_valid_envelope_parsed_directly() {
    let content = load_fixture("discovery/codex-discovery.jsonl");
    let envelope = parse_last_envelope(&content, WorkerState::Seeding)
        .expect("should parse valid envelope");
    assert_eq!(envelope.schema_version, 1);
    assert_eq!(envelope.state, WorkerState::Seeding);
    let output = &envelope.payload["gardener_output"];
    assert_eq!(output["overall_readiness_grade"], "C");
    assert_eq!(output["primary_gap"], "mechanical_guardrails");
}

#[test]
fn discovery_no_envelope_returns_error() {
    let content = load_fixture("discovery/no-envelope.jsonl");
    let err = parse_last_envelope(&content, WorkerState::Seeding)
        .expect_err("should fail without envelope markers");
    let msg = format!("{err}");
    assert!(msg.contains("missing start marker"), "error should mention missing marker: {msg}");
}

#[test]
fn discovery_wrong_state_returns_error() {
    let content = load_fixture("discovery/wrong-state.jsonl");
    let err = parse_last_envelope(&content, WorkerState::Seeding)
        .expect_err("should fail on state mismatch");
    let msg = format!("{err}");
    assert!(msg.contains("state mismatch"), "error should mention state mismatch: {msg}");
}

#[test]
fn discovery_assessment_unknown_has_f_grade() {
    let unknown = DiscoveryAssessment::unknown();
    assert_eq!(unknown.overall_readiness_grade, "F");
    assert_eq!(unknown.primary_gap, "agent_steering");
    assert_eq!(unknown.overall_readiness_score, 10);
}

// --- Non-Interactive Guard Tests ---

#[test]
fn non_interactive_detected_in_ci() {
    let mut env = EnvMap::new();
    env.insert("CI".to_string(), "1".to_string());
    let terminal = FakeTerminal::new(true);
    let reason = is_non_interactive(&env, &terminal);
    assert!(reason.is_some(), "CI environment should be detected as non-interactive");
}

#[test]
fn non_interactive_detected_without_tty() {
    let env = EnvMap::new();
    let terminal = FakeTerminal::new(false);
    let reason = is_non_interactive(&env, &terminal);
    assert!(reason.is_some(), "non-TTY should be detected as non-interactive");
}

#[test]
fn interactive_detected_with_tty_no_ci() {
    let env = EnvMap::new();
    let terminal = FakeTerminal::new(true);
    let reason = is_non_interactive(&env, &terminal);
    assert!(reason.is_none(), "TTY without CI should be interactive, got: {reason:?}");
}
