use gardener::config::CliOverrides;
use gardener::runtime::{
    FakeClock, FakeFileSystem, FakeProcessRunner, FakeTerminal, FileSystem, ProcessOutput,
    ProductionRuntime,
};
use gardener::startup::run_startup_audits;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn runtime_with_files(files: Vec<(&str, &str)>) -> ProductionRuntime {
    let fs = FakeFileSystem::default();
    for (path, content) in files {
        fs.write_string(Path::new(path), content)
            .expect("seed file");
    }
    let process = FakeProcessRunner::default();
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "/repo\n".to_string(),
        stderr: String::new(),
    }));
    process.push_response(Ok(ProcessOutput {
        exit_code: 0,
        stdout: "unknown\n".to_string(),
        stderr: String::new(),
    }));
    ProductionRuntime {
        clock: Arc::new(FakeClock::default()),
        file_system: Arc::new(fs),
        process_runner: Arc::new(process),
        terminal: Arc::new(FakeTerminal::new(true)),
    }
}

#[test]
fn startup_quality_only_writes_quality_doc_when_profile_exists() {
    let cfg_toml = r#"
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
output_path = "profile.toml"
stale_after_commits = 50
discovery_max_turns = 12
[quality_report]
path = "quality.md"
stale_after_days = 7
stale_if_head_commit_differs = true
"#;
    let profile = include_str!("fixtures/triage/expected-profiles/phase03-profile.toml");
    let runtime = runtime_with_files(vec![
        ("/cfg.toml", cfg_toml),
        ("/repo/profile.toml", profile),
    ]);
    let overrides = CliOverrides {
        config_path: Some(PathBuf::from("/cfg.toml")),
        ..CliOverrides::default()
    };
    let (mut cfg, scope) = gardener::config::load_config(
        &overrides,
        Path::new("/repo"),
        runtime.file_system.as_ref(),
        runtime.process_runner.as_ref(),
    )
    .expect("config");

    let summary = run_startup_audits(&runtime, &mut cfg, &scope, false).expect("startup");
    assert!(summary.quality_written);
    assert!(runtime.file_system.exists(Path::new("/repo/quality.md")));
}

#[test]
fn startup_hard_stops_when_profile_missing() {
    let cfg_toml = r#"
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
output_path = "missing-profile.toml"
stale_after_commits = 50
discovery_max_turns = 12
[quality_report]
path = "quality.md"
stale_after_days = 7
stale_if_head_commit_differs = true
"#;
    let runtime = runtime_with_files(vec![("/cfg.toml", cfg_toml)]);
    let overrides = CliOverrides {
        config_path: Some(PathBuf::from("/cfg.toml")),
        ..CliOverrides::default()
    };
    let (mut cfg, scope) = gardener::config::load_config(
        &overrides,
        Path::new("/repo"),
        runtime.file_system.as_ref(),
        runtime.process_runner.as_ref(),
    )
    .expect("config");

    let err = run_startup_audits(&runtime, &mut cfg, &scope, false).expect_err("missing profile");
    assert!(format!("{err}").contains("No repo intelligence profile found"));
}
