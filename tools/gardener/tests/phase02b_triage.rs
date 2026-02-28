use gardener::config::CliOverrides;
use gardener::repo_intelligence::read_profile;
use gardener::runtime::{
    FakeClock, FakeFileSystem, FakeProcessRunner, FakeTerminal, ProcessOutput, ProductionRuntime,
};
use gardener::triage::{run_triage, triage_needed, TriageDecision};
use gardener::triage_agent_detection::{detect_agent, is_non_interactive, EnvMap};
use gardener::triage_discovery::build_discovery_prompt;
use gardener::types::{AgentKind, NonInteractiveReason, RuntimeScope};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}/tests/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
}

fn default_profile_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
        .join(".gardener")
        .join("repo")
        .join("repo-intelligence.toml")
}

fn basic_runtime_for_toml(config_path: &Path, config: &str, tty: bool) -> ProductionRuntime {
    let fs = FakeFileSystem::with_file(config_path, config);
    let process = FakeProcessRunner::default();
    for output in ["/repo\n", "deadbeef\n", "deadbeef\n", "deadbeef\n", "0\n"] {
        process.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: output.to_string(),
            stderr: String::new(),
        }));
    }

    ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process),
        terminal: Arc::new(FakeTerminal::new(tty)),
    }
}

#[test]
fn non_interactive_signals_are_detected() {
    let tty = FakeTerminal::new(true);
    let mut env = EnvMap::new();
    env.insert("CLAUDECODE".to_string(), String::new());
    assert_eq!(
        is_non_interactive(&env, &tty),
        Some(NonInteractiveReason::ClaudeCodeEnv)
    );

    env.clear();
    env.insert("CODEX_THREAD_ID".to_string(), "x".to_string());
    assert_eq!(
        is_non_interactive(&env, &tty),
        Some(NonInteractiveReason::CodexThreadEnv)
    );

    env.clear();
    env.insert("CI".to_string(), "1".to_string());
    assert_eq!(
        is_non_interactive(&env, &tty),
        Some(NonInteractiveReason::CiEnv)
    );

    let non_tty = FakeTerminal::new(false);
    assert_eq!(
        is_non_interactive(&EnvMap::new(), &non_tty),
        Some(NonInteractiveReason::NonTtyStdin)
    );
}

#[test]
fn agent_detection_classifies_fixture_variants() {
    let fs = gardener::runtime::ProductionFileSystem;
    let root = fixture("repos");

    let full = detect_agent(
        &fs,
        &root.join("triage-fully-equipped"),
        &root.join("triage-fully-equipped"),
    );
    assert!(!full.claude_signals.is_empty());
    assert!(!full.codex_signals.is_empty());

    let claude = detect_agent(
        &fs,
        &root.join("triage-claude-only"),
        &root.join("triage-claude-only"),
    );
    assert!(!claude.claude_signals.is_empty());
    assert!(claude.codex_signals.is_empty());

    let codex = detect_agent(
        &fs,
        &root.join("triage-codex-only"),
        &root.join("triage-codex-only"),
    );
    assert!(codex.claude_signals.is_empty());
    assert!(!codex.codex_signals.is_empty());

    let none = detect_agent(
        &fs,
        &root.join("triage-no-agents"),
        &root.join("triage-no-agents"),
    );
    assert!(none.claude_signals.is_empty());
    assert!(none.codex_signals.is_empty());
}

#[test]
fn discovery_prompt_includes_scope_details() {
    let scope = RuntimeScope {
        process_cwd: PathBuf::from("/repo"),
        repo_root: Some(PathBuf::from("/repo")),
        working_dir: PathBuf::from("/repo/sub"),
    };
    let prompt = build_discovery_prompt(&scope);
    assert!(prompt.contains("WORKING DIRECTORY: /repo/sub"));
    assert!(prompt.contains("REPOSITORY ROOT: /repo"));
    assert!(prompt.contains("scope_notes"));
}

#[test]
fn triage_writes_profile_and_triage_needed_logic() {
    let config_path = PathBuf::from("/cfg.toml");
    let config = r#"
[scope]
working_dir = "."
[validation]
command = "npm run validate"
allow_agent_discovery = true
[agent]
default = "codex"
[seeding]
backend = "codex"
model = "gpt-5-codex"
max_turns = 12
[triage]
output_path = ".gardener/repo-intelligence.toml"
stale_after_commits = 50
discovery_max_turns = 12
"#;
    let runtime = basic_runtime_for_toml(&config_path, config, true);
    let overrides = CliOverrides {
        config_path: Some(config_path),
        ..CliOverrides::default()
    };
    let (cfg, scope) = gardener::config::load_config(
        &overrides,
        Path::new("/repo"),
        runtime.file_system.as_ref(),
        runtime.process_runner.as_ref(),
    )
    .expect("cfg");

    let mut env = EnvMap::new();
    let profile = run_triage(&runtime, &scope, &cfg, &env, Some(AgentKind::Codex)).expect("triage");
    assert_eq!(profile.meta.schema_version, 1);

    let path = default_profile_path();
    let loaded = read_profile(runtime.file_system.as_ref(), &path).expect("profile read");
    assert_eq!(loaded.meta.schema_version, 1);

    let decision = triage_needed(&scope, &cfg, &runtime, false).expect("decision");
    assert_eq!(decision, TriageDecision::NotNeeded);

    env.insert("CI".to_string(), "1".to_string());
    let non_interactive = is_non_interactive(&env, runtime.terminal.as_ref());
    assert_eq!(non_interactive, Some(NonInteractiveReason::CiEnv));
}
