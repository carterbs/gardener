use gardener::config::AppConfig;
use gardener::errors::GardenerError;
use gardener::repo_intelligence::read_profile;
use gardener::runtime::{
    FakeClock, FakeFileSystem, FakeProcessRunner, FileSystem, FakeTerminal, ProcessOutput,
    ProductionRuntime, Terminal,
};
use gardener::triage::{profile_path, run_triage, triage_needed, TriageDecision};
use gardener::triage_agent_detection::{detect_agent, DetectedAgent, EnvMap};
use gardener::triage_discovery::{run_discovery, DiscoveryAssessment};
use gardener::types::{AgentKind, RuntimeScope};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct WriteFailingTerminal {
    is_tty: bool,
}

impl Terminal for WriteFailingTerminal {
    fn stdin_is_tty(&self) -> bool {
        self.is_tty
    }

    fn draw(&self, _frame: &str) -> Result<(), GardenerError> {
        Ok(())
    }

    fn write_line(&self, _line: &str) -> Result<(), GardenerError> {
        Err(GardenerError::Io("terminal write failed".to_string()))
    }
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

fn profile_with_head_sha(head_sha: &str) -> String {
    include_str!("fixtures/triage/expected-profiles/phase03-profile.toml")
        .replace("head_sha = \"unknown\"", &format!("head_sha = \"{head_sha}\""))
}

fn discovery_stdout() -> String {
    include_str!("fixtures/agent-responses/discovery/codex-discovery.jsonl").to_string()
}

#[test]
fn triage_needed_unknown_sha_profile_is_never_retriggered() {
    let scope = default_scope();
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: "boom\n".to_string(),
    }));

    let fs = FakeFileSystem::with_file(
        "/repo/.gardener/repo-intelligence.toml",
        profile_with_head_sha("unknown"),
    );
    let runtime = ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process),
        terminal: Arc::new(FakeTerminal::new(true)),
    };

    let decision = triage_needed(&scope, &default_config(), &runtime, false)
        .expect("triage_needed should succeed");
    assert_eq!(decision, TriageDecision::NotNeeded);
}

#[test]
fn triage_needed_git_revparse_failure_uses_unknown_sha() {
    // SWALLOWED: head lookup failure and commit-count failure route to NotNeeded.
    let scope = default_scope();
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: "rev-parse failed\n".to_string(),
    }));
    process.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: "rev-list failed\n".to_string(),
    }));

    let fs = FakeFileSystem::with_file(
        "/repo/.gardener/repo-intelligence.toml",
        profile_with_head_sha("abc123"),
    );
    let runtime = ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process.clone()),
        terminal: Arc::new(FakeTerminal::new(true)),
    };

    let decision = triage_needed(&scope, &default_config(), &runtime, false)
        .expect("triage_needed should succeed");
    assert_eq!(decision, TriageDecision::NotNeeded);
    assert_eq!(process.waits().len(), 2);
}

#[test]
fn triage_needed_commits_since_failure_defaults_not_stale() {
    // SWALLOWED: a failing rev-list call is converted to 0 commits.
    let scope = default_scope();
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "def456\n".to_string(),
        stderr: String::new(),
    }));
    process.push_response(Ok(ProcessOutput {
        exit_code: 128,
        stdout: String::new(),
        stderr: "unknown revision\n".to_string(),
    }));

    let fs = FakeFileSystem::with_file(
        "/repo/.gardener/repo-intelligence.toml",
        profile_with_head_sha("abc123"),
    );
    let runtime = ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process),
        terminal: Arc::new(FakeTerminal::new(true)),
    };

    let decision = triage_needed(&scope, &default_config(), &runtime, false)
        .expect("triage_needed should succeed");
    assert_eq!(decision, TriageDecision::NotNeeded);
}

#[test]
fn profile_with_schema_version_99_is_silently_accepted() {
    let fs = FakeFileSystem::with_file(
        "/repo/.gardener/repo-intelligence.toml",
        include_str!("fixtures/triage/expected-profiles/phase03-profile.toml")
            .replace("schema_version = 1", "schema_version = 99"),
    );
    let runtime = ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(fs),
        process_runner: Arc::new(FakeProcessRunner::default()),
        terminal: Arc::new(FakeTerminal::new(true)),
    };
    let profile = read_profile(
        runtime.file_system.as_ref(),
        &PathBuf::from("/repo/.gardener/repo-intelligence.toml"),
    )
    .expect("profile parse should succeed");
    assert_eq!(profile.meta.schema_version, 99);
}

#[test]
fn detect_agent_both_signals_defaults_to_codex() {
    let fs = FakeFileSystem::default();
    fs.write_string(Path::new("/repo/AGENTS.md"), "agent instructions")
        .expect("write fixture");
    let detected = detect_agent(&fs, &PathBuf::from("/repo"), &PathBuf::from("/repo"));
    assert_eq!(detected.detected, DetectedAgent::Both);

    let chosen_agent = match detected.detected {
        gardener::triage_agent_detection::DetectedAgent::Claude => AgentKind::Claude,
        _ => AgentKind::Codex,
    };
    assert_eq!(chosen_agent, AgentKind::Codex);
}

#[test]
fn run_discovery_process_error_can_be_fallback_to_unknown() {
    let scope = default_scope();
    let process = FakeProcessRunner::default();
    process.push_response(Err(GardenerError::Process("agent process died".to_string())));

    let discovered = run_discovery(&process, &scope, AgentKind::Codex, "gpt-5-codex", 12);
    let fallback = discovered.unwrap_or_else(|_| DiscoveryAssessment::unknown());
    assert_eq!(fallback.overall_readiness_grade, "F");
    assert_eq!(fallback.primary_gap, "agent_steering");
}

#[test]
fn run_discovery_invalid_json_falls_back_to_unknown() {
    let scope = default_scope();
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "<<GARDENER_JSON_START>>\nnot valid json\n<<GARDENER_JSON_END>>\n".to_string(),
        stderr: String::new(),
    }));

    let discovered = run_discovery(&process, &scope, AgentKind::Codex, "gpt-5-codex", 12);
    let fallback = discovered.unwrap_or_else(|_| DiscoveryAssessment::unknown());
    assert_eq!(fallback.overall_readiness_grade, "F");
}

#[test]
fn run_discovery_missing_gardener_output_falls_back_to_unknown() {
    let scope = default_scope();
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout:
            "<<GARDENER_JSON_START>>\n{\"schema_version\":1,\"state\":\"seeding\",\"payload\":{\"wrong_key\":\"value\"}}\n<<GARDENER_JSON_END>>\n"
                .to_string(),
        stderr: String::new(),
    }));

    let discovered = run_discovery(&process, &scope, AgentKind::Codex, "gpt-5-codex", 12);
    let fallback = discovered.unwrap_or_else(|_| DiscoveryAssessment::unknown());
    assert_eq!(fallback.overall_readiness_grade, "F");
}

#[test]
fn discovery_lost_when_interview_write_line_fails() {
    // B6: interview fallback write failures abort triage before profile persistence.
    let scope = default_scope();
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: discovery_stdout(),
        stderr: String::new(),
    }));
    let runtime = ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(FakeFileSystem::default()),
        process_runner: Arc::new(process),
        terminal: Arc::new(WriteFailingTerminal { is_tty: true }),
    };
    let cfg = default_config();
    let env = EnvMap::new();

    let err = run_triage(&runtime, &scope, &cfg, &env, None)
        .expect_err("write failure should fail triage");
    assert!(format!("{err}").contains("terminal write failed"));
    let path = profile_path(&scope, &cfg);
    assert!(!runtime.file_system.exists(&path));
}

#[test]
fn unknown_sha_written_to_profile_after_git_failure() {
    // B1: head lookup failure is persisted as "unknown" and still writes a profile.
    let scope = default_scope();
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: discovery_stdout(),
        stderr: String::new(),
    }));
    process.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: "boom".to_string(),
    }));
    let runtime = ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(FakeFileSystem::default()),
        process_runner: Arc::new(process),
        terminal: Arc::new(FakeTerminal::new(true)),
    };
    let cfg = default_config();
    let env = EnvMap::new();

    let profile = run_triage(&runtime, &scope, &cfg, &env, None)
        .expect("run_triage should persist profile with unknown head_sha");
    assert_eq!(profile.meta.head_sha, "unknown");
    let path = profile_path(&scope, &cfg);
    let persisted = read_profile(runtime.file_system.as_ref(), &path).expect("profile persisted");
    assert_eq!(persisted.meta.head_sha, "unknown");
}
