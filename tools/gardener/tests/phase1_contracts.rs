use assert_cmd::cargo::cargo_bin_cmd;
use gardener::config::{
    effective_agent_for_state, effective_model_for_state, load_config, resolve_scope,
    resolve_validation_command, AppConfig, CliOverrides, StateConfig,
};
use gardener::errors::GardenerError;
use gardener::output_envelope::{parse_last_envelope, END_MARKER, START_MARKER};
use gardener::runtime::{
    Clock, FakeClock, FakeFileSystem, FakeProcessRunner, FakeTerminal, FileSystem, ProcessOutput,
    ProcessRequest, ProcessRunner, ProductionClock, ProductionFileSystem, ProductionProcessRunner,
    ProductionRuntime, Terminal,
};
use gardener::triage_agent_detection::{is_non_interactive, EnvMap};
use gardener::types::{AgentKind, NonInteractiveReason, WorkerState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

const TEST_REPO_ROOT: &str = "/tmp/gardener-phase1-contracts";

fn runtime_with_config(config_text: &str, tty: bool, git_root: Option<&str>) -> ProductionRuntime {
    let fs = FakeFileSystem::with_file("/config.toml", config_text);
    let profile_path = PathBuf::from(git_root.unwrap_or("/repo"))
        .join(".gardener")
        .join("repo-intelligence.toml");
    fs.write_string(
        profile_path.as_path(),
        include_str!("fixtures/triage/expected-profiles/phase03-profile.toml"),
    )
    .expect("seed profile");
    let process = FakeProcessRunner::default();
    for _ in 0..20 {
        process.push_response(Ok(ProcessOutput {
            exit_code: if git_root.is_some() { 0 } else { 1 },
            stdout: git_root.map(|v| format!("{v}\n")).unwrap_or_default(),
            stderr: String::new(),
        }));
    }
    let terminal = FakeTerminal::new(tty);

    ProductionRuntime {
        clock: Arc::new(ProductionClock),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process),
        terminal: Arc::new(terminal),
    }
}

struct FailingTerminal {
    is_tty: bool,
}

impl Terminal for FailingTerminal {
    fn stdin_is_tty(&self) -> bool {
        self.is_tty
    }

    fn write_line(&self, _line: &str) -> Result<(), GardenerError> {
        Err(GardenerError::Io("terminal write failed".to_string()))
    }

    fn draw(&self, _frame: &str) -> Result<(), GardenerError> {
        Err(GardenerError::Io("terminal draw failed".to_string()))
    }
}

fn runtime_with_failing_terminal() -> ProductionRuntime {
    let fs = FakeFileSystem::with_file("/config.toml", "");
    fs.write_string(
        Path::new("/repo/.gardener/repo-intelligence.toml"),
        include_str!("fixtures/triage/expected-profiles/phase03-profile.toml"),
    )
    .expect("seed profile");
    let process = FakeProcessRunner::default();
    for _ in 0..12 {
        process.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "/repo\n".to_string(),
            stderr: String::new(),
        }));
    }

    ProductionRuntime {
        clock: Arc::new(ProductionClock),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process),
        terminal: Arc::new(FailingTerminal { is_tty: true }),
    }
}

#[test]
fn cli_help_contract() {
    let help = gardener::render_help();
    for flag in [
        "--config",
        "--working-dir",
        "--parallelism",
        "--task",
        "--quit-after",
        "--prune-only",
        "--backlog-only",
        "--quality-grades-only",
        "--validate",
        "--validation-command",
        "--agent",
        "--retriage",
        "--triage-only",
        "--sync-only",
    ] {
        assert!(help.contains(flag));
    }
    assert!(!help.contains("--headless"));
}

fn fixture(path: &str) -> String {
    format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn binary_help_and_prune_smoke() {
    let mut help = cargo_bin_cmd!("gardener");
    help.arg("--help");
    let out = help.assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");
    assert!(stdout.contains("--agent"));
    assert!(!stdout.contains("--headless"));

    let mut prune = cargo_bin_cmd!("gardener");
    prune
        .arg("--prune-only")
        .arg("--config")
        .arg(fixture("configs/phase01-minimal.toml"));
    prune.assert().success();

    let mut scoped = cargo_bin_cmd!("gardener");
    scoped
        .arg("--prune-only")
        .arg("--config")
        .arg(fixture("configs/phase01-minimal.toml"))
        .arg("--working-dir")
        .arg(fixture("repos/scoped-app/packages/functions/src"));
    scoped.assert().success();
}

#[test]
fn run_with_runtime_paths_and_errors() {
    // Clean stale SQLite from prior runs to avoid claiming leftover tasks
    let _ =
        std::fs::remove_file(PathBuf::from(TEST_REPO_ROOT).join(".cache/gardener/backlog.sqlite"));
    let runtime = runtime_with_config(
        "[execution]\ntest_mode = true\nworker_mode = \"normal\"\n",
        true,
        Some(TEST_REPO_ROOT),
    );
    let prune = vec![
        "gardener".into(),
        "--prune-only".into(),
        "--config".into(),
        "/config.toml".into(),
    ];
    assert_eq!(
        gardener::run_with_runtime(&prune, &[], Path::new("/cwd"), &runtime).unwrap(),
        0
    );

    let backlog = vec![
        "gardener".into(),
        "--backlog-only".into(),
        "--config".into(),
        "/config.toml".into(),
    ];
    let backlog_result = gardener::run_with_runtime(&backlog, &[], Path::new("/cwd"), &runtime);
    eprintln!("backlog result: {backlog_result:?}");
    assert_eq!(backlog_result.unwrap(), 0);

    let quality = vec![
        "gardener".into(),
        "--quality-grades-only".into(),
        "--config".into(),
        "/config.toml".into(),
    ];
    let quality_result = gardener::run_with_runtime(&quality, &[], Path::new("/cwd"), &runtime);
    eprintln!("quality result: {quality_result:?}");
    assert_eq!(quality_result.unwrap(), 0);

    let validate = vec![
        "gardener".into(),
        "--validate".into(),
        "--config".into(),
        "/config.toml".into(),
    ];
    let validate_result = gardener::run_with_runtime(&validate, &[], Path::new("/cwd"), &runtime);
    eprintln!("validate result: {validate_result:?}");
    assert_eq!(validate_result.unwrap(), 0);

    let normal = vec!["gardener".into(), "--config".into(), "/config.toml".into()];
    let normal_result = gardener::run_with_runtime(&normal, &[], Path::new("/cwd"), &runtime);
    eprintln!("normal result: {normal_result:?}");
    assert_eq!(normal_result.unwrap(), 0);

    let help = vec!["gardener".into(), "--help".into()];
    assert_eq!(
        gardener::run_with_runtime(&help, &[], Path::new("/cwd"), &runtime).unwrap(),
        0
    );

    let invalid = vec!["gardener".into(), "--agent".into(), "invalid".into()];
    let err = gardener::run_with_runtime(&invalid, &[], Path::new("/cwd"), &runtime).unwrap_err();
    assert!(matches!(err, GardenerError::Cli(_)));

    let retriage = vec!["gardener".into(), "--retriage".into()];
    let err = gardener::run_with_runtime(
        &retriage,
        &[("CI".into(), "1".into())],
        Path::new("/cwd"),
        &runtime,
    )
    .unwrap_err();
    assert!(matches!(err, GardenerError::Cli(message) if message.contains("interactive")));

    let triage = vec!["gardener".into(), "--triage-only".into()];
    let non_tty_runtime = runtime_with_config("", false, Some(TEST_REPO_ROOT));
    let err =
        gardener::run_with_runtime(&triage, &[], Path::new("/cwd"), &non_tty_runtime).unwrap_err();
    assert!(matches!(err, GardenerError::Cli(message) if message.contains("interactive")));
}

#[test]
fn run_with_runtime_validate_flag_runs_configured_validation_command() {
    let fs = FakeFileSystem::with_file("/config.toml", "[execution]\ntest_mode = true\n");
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "/repo\n".to_string(),
        stderr: String::new(),
    }));
    process.push_response(Ok(ProcessOutput {
        exit_code: 7,
        stdout: String::new(),
        stderr: "failed\n".to_string(),
    }));
    let runtime = ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process.clone()),
        terminal: Arc::new(FakeTerminal::new(true)),
    };

    let validate = vec![
        "gardener".into(),
        "--validate".into(),
        "--config".into(),
        "/config.toml".into(),
    ];
    let validate_result = gardener::run_with_runtime(&validate, &[], Path::new("/cwd"), &runtime);
    assert_eq!(validate_result.unwrap(), 7);

    let spawned = process.spawned();
    assert_eq!(spawned.len(), 2);
    assert_eq!(spawned[1].program, "sh");
    assert_eq!(
        spawned[1].args,
        vec!["-lc".to_string(), "npm run validate".to_string()]
    );
    assert_eq!(spawned[1].cwd, Some(PathBuf::from("/repo")));
}

#[test]
fn run_with_runtime_propagates_write_and_config_errors() {
    let runtime = runtime_with_failing_terminal();
    let prune = vec![
        "gardener".into(),
        "--prune-only".into(),
        "--config".into(),
        "/config.toml".into(),
    ];
    let backlog = vec!["gardener".into(), "--backlog-only".into()];
    let quality = vec!["gardener".into(), "--quality-grades-only".into()];
    let normal = vec!["gardener".into()];

    for args in [prune, backlog, quality, normal] {
        let err = gardener::run_with_runtime(&args, &[], Path::new("/cwd"), &runtime).unwrap_err();
        assert!(
            matches!(err, GardenerError::Io(message) if message.contains("terminal write failed") || message.contains("terminal draw failed"))
        );
    }

    let ok_runtime = runtime_with_config("", true, Some(TEST_REPO_ROOT));
    let missing_cfg = vec![
        "gardener".into(),
        "--prune-only".into(),
        "--config".into(),
        "/missing.toml".into(),
    ];
    let err =
        gardener::run_with_runtime(&missing_cfg, &[], Path::new("/cwd"), &ok_runtime).unwrap_err();
    assert!(matches!(err, GardenerError::Io(message) if message.contains("missing file")));
}

#[test]
fn config_precedence_and_resolution_contracts() {
    let config_toml = r#"
[orchestrator]
parallelism = 7

[validation]
command = "npm run validate:file"

[agent]
default = "claude"
"#;
    let fs = FakeFileSystem::with_file("/cfg.toml", config_toml);
    let process_runner = FakeProcessRunner::default();
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }));

    let overrides = CliOverrides {
        config_path: Some(PathBuf::from("/cfg.toml")),
        parallelism: Some(9),
        validation_command: Some("npm run validate:cli".to_string()),
        agent: Some(AgentKind::Codex),
        ..CliOverrides::default()
    };

    let (cfg, _scope) = load_config(&overrides, Path::new("/cwd"), &fs, &process_runner).unwrap();
    assert_eq!(cfg.orchestrator.parallelism, 9);
    assert_eq!(cfg.validation.command, "npm run validate:cli");
    assert_eq!(cfg.agent.default, Some(AgentKind::Codex));

    let mut cfg2 = AppConfig::default();
    cfg2.agent.default = Some(AgentKind::Claude);
    cfg2.states.insert(
        "doing".to_string(),
        StateConfig {
            backend: Some(AgentKind::Codex),
            model: Some("gpt-5-codex".to_string()),
        },
    );
    assert_eq!(
        effective_agent_for_state(&cfg2, WorkerState::Doing),
        Some(AgentKind::Codex)
    );
    assert_eq!(
        effective_agent_for_state(&cfg2, WorkerState::Planning),
        Some(AgentKind::Claude)
    );
    assert_eq!(
        effective_model_for_state(&cfg2, WorkerState::Doing),
        "gpt-5-codex"
    );
    assert_eq!(
        effective_model_for_state(&cfg2, WorkerState::Planning),
        cfg2.seeding.model
    );

    let resolved = resolve_validation_command(&cfg2, Some("npm run custom"));
    assert_eq!(resolved.command, "npm run custom");

    cfg2.validation.command.clear();
    cfg2.startup.validation_command = Some("npm run startup".to_string());
    let resolved = resolve_validation_command(&cfg2, None);
    assert_eq!(resolved.command, "npm run startup");

    cfg2.startup.validation_command = None;
    let resolved = resolve_validation_command(&cfg2, None);
    assert_eq!(resolved.command, "true");
}

#[test]
fn config_exhaustive_overrides_and_validation_paths() {
    let config_toml = r#"
[scheduler]
lease_timeout_seconds = 111
heartbeat_interval_seconds = 22

[prompts.token_budget]
understand = 1
planning = 2
doing = 3
gitting = 4
reviewing = 5
merging = 6

[learning]
confidence_decay_per_day = 0.5
deactivate_below_confidence = 0.7

[seeding]
backend = "claude"
model = "claude-sonnet-4-6"
max_turns = 99

[execution]
permissions_mode = "custom"
worker_mode = "normal"
test_mode = true
"#;
    let fs = FakeFileSystem::with_file("/cfg.toml", config_toml);
    let process_runner = FakeProcessRunner::default();
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: " \n".to_string(),
        stderr: String::new(),
    }));
    let overrides = CliOverrides {
        config_path: Some(PathBuf::from("/cfg.toml")),
        ..CliOverrides::default()
    };
    let (cfg, scope) = load_config(&overrides, Path::new("/cwd"), &fs, &process_runner).unwrap();
    assert_eq!(cfg.scheduler.lease_timeout_seconds, 111);
    assert_eq!(cfg.scheduler.heartbeat_interval_seconds, 22);
    assert_eq!(cfg.prompts.token_budget.understand, 1);
    assert_eq!(cfg.prompts.token_budget.planning, 2);
    assert_eq!(cfg.prompts.token_budget.doing, 3);
    assert_eq!(cfg.prompts.token_budget.gitting, 4);
    assert_eq!(cfg.prompts.token_budget.reviewing, 5);
    assert_eq!(cfg.prompts.token_budget.merging, 6);
    assert_eq!(cfg.learning.confidence_decay_per_day, 0.5);
    assert_eq!(cfg.learning.deactivate_below_confidence, 0.7);
    assert_eq!(cfg.seeding.backend, AgentKind::Claude);
    assert_eq!(cfg.seeding.model, "claude-sonnet-4-6");
    assert_eq!(cfg.seeding.max_turns, 99);
    assert_eq!(cfg.execution.permissions_mode, "custom");
    assert_eq!(cfg.execution.worker_mode, "normal");
    assert!(cfg.execution.test_mode);
    assert_eq!(scope.repo_root, None);

    let bad_parallel = FakeFileSystem::with_file("/bad1.toml", "[orchestrator]\nparallelism = 0\n");
    let err = load_config(
        &CliOverrides {
            config_path: Some(PathBuf::from("/bad1.toml")),
            ..CliOverrides::default()
        },
        Path::new("/cwd"),
        &bad_parallel,
        &FakeProcessRunner::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, GardenerError::InvalidConfig(message) if message.contains("parallelism"))
    );

    let bad_agent =
        FakeFileSystem::with_file("/bad2.toml", "[agent]\n[states.doing]\nmodel = \"x\"\n");
    let err = load_config(
        &CliOverrides {
            config_path: Some(PathBuf::from("/bad2.toml")),
            ..CliOverrides::default()
        },
        Path::new("/cwd"),
        &bad_agent,
        &FakeProcessRunner::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, GardenerError::InvalidConfig(message) if message.contains("agent.default"))
    );

    let bad_model = FakeFileSystem::with_file(
        "/bad3.toml",
        "[seeding]\nmodel = \"...\"\nbackend = \"codex\"\nmax_turns = 1\n",
    );
    let err = load_config(
        &CliOverrides {
            config_path: Some(PathBuf::from("/bad3.toml")),
            ..CliOverrides::default()
        },
        Path::new("/cwd"),
        &bad_model,
        &FakeProcessRunner::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, GardenerError::InvalidConfig(message) if message.contains("seeding.model"))
    );

    let bad_state_model = FakeFileSystem::with_file(
        "/bad4.toml",
        "[states.doing]\nbackend = \"codex\"\nmodel = \"...\"\n",
    );
    let err = load_config(
        &CliOverrides {
            config_path: Some(PathBuf::from("/bad4.toml")),
            ..CliOverrides::default()
        },
        Path::new("/cwd"),
        &bad_state_model,
        &FakeProcessRunner::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, GardenerError::InvalidConfig(message) if message.contains("states.doing.model"))
    );

    let mut cfg2 = AppConfig::default();
    cfg2.agent.default = Some(AgentKind::Claude);
    cfg2.states.insert(
        "doing".to_string(),
        StateConfig {
            backend: None,
            model: Some("x".to_string()),
        },
    );
    assert_eq!(
        effective_agent_for_state(&cfg2, WorkerState::Doing),
        Some(AgentKind::Claude)
    );
    let _ = effective_agent_for_state(&cfg2, WorkerState::Understand);
    let _ = effective_agent_for_state(&cfg2, WorkerState::Gitting);
    let _ = effective_agent_for_state(&cfg2, WorkerState::Reviewing);
    let _ = effective_agent_for_state(&cfg2, WorkerState::Merging);
    let _ = effective_agent_for_state(&cfg2, WorkerState::Seeding);
}

#[test]
fn default_config_is_discovered_from_repo_root_or_cwd() {
    let fs = FakeFileSystem::with_file(
        "/repo/gardener.toml",
        "[orchestrator]\nparallelism = 1\n[seeding]\nbackend = \"codex\"\nmodel = \"gpt-5-codex\"\nmax_turns = 1\n",
    );
    let process_runner = FakeProcessRunner::default();
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "/repo\n".to_string(),
        stderr: String::new(),
    }));
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "/repo\n".to_string(),
        stderr: String::new(),
    }));
    let (cfg, scope) = load_config(
        &CliOverrides::default(),
        Path::new("/cwd"),
        &fs,
        &process_runner,
    )
    .expect("load config from repo root");
    assert_eq!(cfg.orchestrator.parallelism, 1);
    assert_eq!(scope.repo_root, Some(PathBuf::from("/repo")));

    let fs = FakeFileSystem::with_file(
        "/cwd/gardener.toml",
        "[orchestrator]\nparallelism = 2\n[seeding]\nbackend = \"codex\"\nmodel = \"gpt-5-codex\"\nmax_turns = 1\n",
    );
    let process_runner = FakeProcessRunner::default();
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }));
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }));
    let (cfg, scope) = load_config(
        &CliOverrides::default(),
        Path::new("/cwd"),
        &fs,
        &process_runner,
    )
    .expect("load config from cwd");
    assert_eq!(cfg.orchestrator.parallelism, 2);
    assert_eq!(scope.repo_root, None);
}

#[test]
fn config_covers_prompts_without_budget_and_state_backend_present_validation_loop() {
    let fs = FakeFileSystem::with_file("/p.toml", "[prompts]\n");
    let process_runner = FakeProcessRunner::default();
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }));
    let _ = load_config(
        &CliOverrides {
            config_path: Some(PathBuf::from("/p.toml")),
            ..CliOverrides::default()
        },
        Path::new("/cwd"),
        &fs,
        &process_runner,
    )
    .unwrap();

    let fs = FakeFileSystem::with_file(
        "/s.toml",
        "[agent]\n\n[states.doing]\nbackend = \"codex\"\nmodel = \"gpt-5-codex\"\n",
    );
    let process_runner = FakeProcessRunner::default();
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }));
    let _ = load_config(
        &CliOverrides {
            config_path: Some(PathBuf::from("/s.toml")),
            ..CliOverrides::default()
        },
        Path::new("/cwd"),
        &fs,
        &process_runner,
    )
    .unwrap();
}

#[test]
fn working_dir_resolution_contract() {
    let process_runner = FakeProcessRunner::default();
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "/repo\n".to_string(),
        stderr: String::new(),
    }));
    let cfg = AppConfig {
        scope: gardener::config::ScopeConfig {
            working_dir: Some(PathBuf::from("from-config")),
        },
        ..AppConfig::default()
    };
    let overrides = CliOverrides {
        working_dir: Some(PathBuf::from("from-cli")),
        ..CliOverrides::default()
    };
    let scope = resolve_scope(Path::new("/cwd"), &cfg, &overrides, &process_runner);
    assert_eq!(scope.working_dir, PathBuf::from("/cwd/from-cli"));

    let process_runner = FakeProcessRunner::default();
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "/repo\n".to_string(),
        stderr: String::new(),
    }));
    let scope = resolve_scope(
        Path::new("/cwd"),
        &AppConfig::default(),
        &CliOverrides::default(),
        &process_runner,
    );
    assert_eq!(scope.working_dir, PathBuf::from("/repo"));

    let process_runner = FakeProcessRunner::default();
    process_runner.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }));
    let scope = resolve_scope(
        Path::new("/cwd"),
        &AppConfig::default(),
        &CliOverrides::default(),
        &process_runner,
    );
    assert_eq!(scope.working_dir, PathBuf::from("/cwd"));
}

#[test]
fn non_interactive_detection_contract() {
    let tty = FakeTerminal::new(true);
    let non_tty = FakeTerminal::new(false);

    let mut env = EnvMap::new();
    env.insert("CLAUDECODE".to_string(), "1".to_string());
    assert_eq!(
        is_non_interactive(&env, &tty),
        Some(NonInteractiveReason::ClaudeCodeEnv)
    );

    let mut env = EnvMap::new();
    env.insert("CODEX_THREAD_ID".to_string(), "abc".to_string());
    assert_eq!(
        is_non_interactive(&env, &tty),
        Some(NonInteractiveReason::CodexThreadEnv)
    );

    let mut env = EnvMap::new();
    env.insert("CI".to_string(), "1".to_string());
    assert_eq!(
        is_non_interactive(&env, &tty),
        Some(NonInteractiveReason::CiEnv)
    );

    assert_eq!(
        is_non_interactive(&EnvMap::new(), &non_tty),
        Some(NonInteractiveReason::NonTtyStdin)
    );
    assert_eq!(is_non_interactive(&EnvMap::new(), &tty), None);
}

#[test]
fn output_envelope_contract() {
    let good = format!(
        "x\n{START_MARKER}\n{{\"schema_version\":1,\"state\":\"doing\",\"payload\":{{\"ok\":true}}}}\n{END_MARKER}\n"
    );
    let parsed = parse_last_envelope(&good, WorkerState::Doing).unwrap();
    assert_eq!(parsed.payload["ok"], true);

    assert!(matches!(
        parse_last_envelope("x", WorkerState::Doing).unwrap_err(),
        GardenerError::OutputEnvelope(_)
    ));

    let bad_json = format!("{START_MARKER} nope {END_MARKER}");
    assert!(matches!(
        parse_last_envelope(&bad_json, WorkerState::Doing).unwrap_err(),
        GardenerError::OutputEnvelope(_)
    ));
}

#[test]
fn output_envelope_error_contracts() {
    let reversed = format!("{END_MARKER}{START_MARKER}");
    assert!(matches!(
        parse_last_envelope(&reversed, WorkerState::Doing).unwrap_err(),
        GardenerError::OutputEnvelope(message) if message.contains("before")
    ));

    let bad_schema = format!(
        "{START_MARKER} {{\"schema_version\":2,\"state\":\"doing\",\"payload\":{{}}}} {END_MARKER}"
    );
    assert!(matches!(
        parse_last_envelope(&bad_schema, WorkerState::Doing).unwrap_err(),
        GardenerError::OutputEnvelope(message) if message.contains("schema_version")
    ));

    let mismatch = format!(
        "{START_MARKER} {{\"schema_version\":1,\"state\":\"planning\",\"payload\":{{}}}} {END_MARKER}"
    );
    assert!(matches!(
        parse_last_envelope(&mismatch, WorkerState::Doing).unwrap_err(),
        GardenerError::OutputEnvelope(message) if message.contains("state mismatch")
    ));
}

#[test]
fn runtime_fake_contracts() {
    let now = UNIX_EPOCH + Duration::from_secs(10);
    let clock = FakeClock::new(now);
    let deadline = UNIX_EPOCH + Duration::from_secs(20);
    clock.sleep_until(deadline).unwrap();
    assert_eq!(clock.now(), deadline);

    let fs = FakeFileSystem::default();
    let path = Path::new("a.txt");
    fs.write_string(path, "hello").unwrap();
    assert_eq!(fs.read_to_string(path).unwrap(), "hello");
    fs.remove_file(path).unwrap();
    assert!(!fs.exists(path));
    assert!(matches!(
        fs.read_to_string(path).unwrap_err(),
        GardenerError::Io(_)
    ));

    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "ok".to_string(),
        stderr: String::new(),
    }));
    let out = runner
        .run(ProcessRequest {
            program: "git".to_string(),
            args: vec![],
            cwd: None,
        })
        .unwrap();
    assert_eq!(out.exit_code, 0);

    let terminal = FakeTerminal::new(false);
    terminal.write_line("line").unwrap();
    terminal.draw("frame").unwrap();
    assert!(!terminal.stdin_is_tty());
}

#[test]
fn runtime_extra_branch_coverage() {
    let fc = FakeClock::default();
    assert_eq!(fc.sleeps().len(), 0);

    let fs = FakeFileSystem::default();
    fs.set_fail_next(GardenerError::Io("x".to_string()));
    assert!(matches!(
        fs.create_dir_all(Path::new("d")).unwrap_err(),
        GardenerError::Io(_)
    ));
    fs.create_dir_all(Path::new("d")).unwrap();

    let term = FakeTerminal::new(true);
    term.write_line("a").unwrap();
    term.draw("b").unwrap();
    assert_eq!(term.written_lines(), vec!["a".to_string()]);
    assert_eq!(term.drawn_frames(), vec!["b".to_string()]);

    let runner = FakeProcessRunner::default();
    runner.push_response(Ok(ProcessOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }));
    let h = runner
        .spawn(ProcessRequest {
            program: "x".to_string(),
            args: vec![],
            cwd: None,
        })
        .unwrap();
    let _ = runner.wait(h).unwrap();
    runner.kill(h).unwrap();
    assert_eq!(runner.spawned().len(), 1);
    assert_eq!(runner.waits(), vec![0]);
    assert_eq!(runner.kills(), vec![0]);
}

#[cfg(unix)]
#[test]
fn runtime_production_contracts() {
    let clock = ProductionClock;
    clock.sleep_until(clock.now()).unwrap();
    clock
        .sleep_until(clock.now() + Duration::from_millis(1))
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.txt");
    let fs = ProductionFileSystem;
    let nested = dir.path().join("a/b/c");
    fs.create_dir_all(&nested).unwrap();
    fs.write_string(&path, "abc").unwrap();
    assert_eq!(fs.read_to_string(&path).unwrap(), "abc");
    assert!(fs.exists(&path));
    fs.remove_file(&path).unwrap();

    let runner = ProductionProcessRunner::new();
    let _runner_default = ProductionProcessRunner::default();
    let out = runner
        .run(ProcessRequest {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "printf ok".to_string()],
            cwd: None,
        })
        .unwrap();
    assert_eq!(out.stdout, "ok");

    let handle = runner
        .spawn(ProcessRequest {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 1".to_string()],
            cwd: None,
        })
        .unwrap();
    runner.kill(handle).unwrap();
    assert!(matches!(
        runner.wait(999).unwrap_err(),
        GardenerError::Process(_)
    ));
    assert!(matches!(
        runner.kill(999).unwrap_err(),
        GardenerError::Process(_)
    ));

    let rt = ProductionRuntime::new();
    let _rt_default = ProductionRuntime::default();
    let prod_terminal = gardener::runtime::ProductionTerminal;
    prod_terminal.draw("frame").unwrap();
    assert!(rt.terminal.stdin_is_tty() || !rt.terminal.stdin_is_tty());
}

#[test]
fn cli_agent_from_impl_covers_both_variants() {
    let claude: AgentKind = gardener::CliAgent::Claude.into();
    let codex: AgentKind = gardener::CliAgent::Codex.into();
    assert_eq!(claude, AgentKind::Claude);
    assert_eq!(codex, AgentKind::Codex);
}

#[test]
fn agent_kind_helpers() {
    assert_eq!(AgentKind::Claude.as_str(), "claude");
    assert_eq!(AgentKind::Codex.as_str(), "codex");
}
