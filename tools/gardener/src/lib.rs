pub mod agent;
pub mod backlog_snapshot;
pub mod backlog_store;
pub mod config;
pub mod errors;
pub mod fsm;
pub mod learning_loop;
pub mod output_envelope;
pub mod postmerge_analysis;
pub mod postmortem;
pub mod pr_audit;
pub mod priority;
pub mod protocol;
pub mod prompt_context;
pub mod prompt_knowledge;
pub mod prompt_registry;
pub mod prompts;
pub mod quality_domain_catalog;
pub mod quality_evidence;
pub mod quality_grades;
pub mod quality_scoring;
pub mod repo_intelligence;
pub mod runtime;
pub mod scheduler;
pub mod seed_runner;
pub mod seeding;
pub mod startup;
pub mod task_identity;
pub mod triage;
pub mod triage_agent_detection;
pub mod triage_discovery;
pub mod triage_interview;
pub mod types;
pub mod worker;
pub mod worker_identity;
pub mod worker_pool;
pub mod worktree_audit;

use backlog_store::BacklogStore;
use agent::factory::AdapterFactory;
use agent::{probe_and_persist, validate_model};
use clap::{error::ErrorKind, CommandFactory, Parser, ValueEnum};
use config::{load_config, resolve_validation_command, CliOverrides};
use errors::GardenerError;
use runtime::ProductionRuntime;
use startup::run_startup_audits;
use triage::ensure_profile_for_run;
use triage_agent_detection::{is_non_interactive, EnvMap};
use types::{AgentKind, RuntimeScope, ValidationCommandResolution};
use worker_pool::{run_worker_pool_fsm, run_worker_pool_stub};

#[derive(Debug, Clone, Parser)]
#[command(name = "gardener")]
#[command(about = "Rust runtime skeleton for Gardener")]
pub struct Cli {
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,
    #[arg(long)]
    pub working_dir: Option<std::path::PathBuf>,
    #[arg(long)]
    pub parallelism: Option<u32>,
    #[arg(long)]
    pub task: Option<String>,
    #[arg(long)]
    pub target: Option<u32>,
    #[arg(long, default_value_t = false)]
    pub prune_only: bool,
    #[arg(long, default_value_t = false)]
    pub backlog_only: bool,
    #[arg(long, default_value_t = false)]
    pub quality_grades_only: bool,
    #[arg(long)]
    pub validation_command: Option<String>,
    #[arg(long, value_enum)]
    pub agent: Option<CliAgent>,
    #[arg(long, default_value_t = false)]
    pub retriage: bool,
    #[arg(long, default_value_t = false)]
    pub triage_only: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliAgent {
    Claude,
    Codex,
}

impl From<CliAgent> for AgentKind {
    fn from(value: CliAgent) -> Self {
        match value {
            CliAgent::Claude => AgentKind::Claude,
            CliAgent::Codex => AgentKind::Codex,
        }
    }
}

pub struct StartupSnapshot {
    pub scope: RuntimeScope,
    pub validation: ValidationCommandResolution,
}

pub fn run() -> Result<i32, GardenerError> {
    let args = std::env::args_os().collect::<Vec<_>>();
    let env = std::env::vars_os().collect::<Vec<_>>();
    let cwd = std::env::current_dir().map_err(|e| GardenerError::Io(e.to_string()))?;
    let runtime = ProductionRuntime::new();
    run_with_runtime(&args, &env, &cwd, &runtime)
}

pub fn run_with_runtime(
    args: &[std::ffi::OsString],
    env: &[(std::ffi::OsString, std::ffi::OsString)],
    cwd: &std::path::Path,
    runtime: &ProductionRuntime,
) -> Result<i32, GardenerError> {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => match error.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                print!("{error}");
                return Ok(0);
            }
            _ => return Err(GardenerError::Cli(error.to_string())),
        },
    };

    let env_map = env_to_map(env);

    if cli.retriage && is_non_interactive(&env_map, runtime.terminal.as_ref()).is_some() {
        return Err(GardenerError::Cli(
            "--retriage requires an interactive terminal.".to_string(),
        ));
    }
    if cli.triage_only && is_non_interactive(&env_map, runtime.terminal.as_ref()).is_some() {
        return Err(GardenerError::Cli(
            "Triage requires a human and cannot run non-interactively.".to_string(),
        ));
    }

    let overrides = CliOverrides {
        config_path: cli.config.clone(),
        working_dir: cli.working_dir.clone(),
        parallelism: cli.parallelism,
        task: cli.task.clone(),
        target: cli.target,
        prune_only: cli.prune_only,
        backlog_only: cli.backlog_only,
        quality_grades_only: cli.quality_grades_only,
        validation_command: cli.validation_command.clone(),
        agent: cli.agent.map(Into::into),
        retriage: cli.retriage,
        triage_only: cli.triage_only,
    };

    let (cfg, scope) = load_config(
        &overrides,
        cwd,
        runtime.file_system.as_ref(),
        runtime.process_runner.as_ref(),
    )?;

    if let (Some(agent), Some(config_path)) = (cli.agent, cli.config.as_ref()) {
        persist_agent_default(
            runtime.file_system.as_ref(),
            config_path.as_path(),
            AgentKind::from(agent),
        )?;
    }

    let validation = resolve_validation_command(&cfg, cli.validation_command.as_deref());
    let startup = StartupSnapshot { scope, validation };

    if cli.triage_only || cli.retriage {
        let _profile = ensure_profile_for_run(
            runtime,
            &startup.scope,
            &cfg,
            &env_map,
            cli.retriage,
            cli.agent.map(Into::into),
        )?;
        runtime.terminal.write_line("triage complete")?;
        return Ok(0);
    }

    if cli.prune_only {
        runtime.terminal.write_line(&format!(
            "phase1 prune-only: scope={} validation={}",
            startup.scope.working_dir.display(),
            startup.validation.command
        ))?;
        return Ok(0);
    }

    if cli.backlog_only {
        let mut cfg_for_startup = cfg.clone();
        let _ = run_startup_audits(runtime, &mut cfg_for_startup, &startup.scope, true)?;
        runtime.terminal.write_line("phase3 backlog-only")?;
        return Ok(0);
    }

    if cli.quality_grades_only {
        let mut cfg_for_startup = cfg.clone();
        let _ = run_startup_audits(runtime, &mut cfg_for_startup, &startup.scope, false)?;
        runtime.terminal.write_line("phase3 quality-grades-only")?;
        return Ok(0);
    }

    if let Some(target) = cli.target {
        let mut cfg_for_startup = cfg.clone();
        validate_model(&cfg_for_startup.seeding.model)?;
        if !cfg_for_startup.execution.test_mode {
            let factory = AdapterFactory::with_defaults();
            let mut active = Vec::new();
            if let Some(adapter) = factory.get(cfg_for_startup.seeding.backend) {
                active.push(adapter);
            } else {
                return Err(GardenerError::InvalidConfig(format!(
                    "no adapter registered for backend {:?}",
                    cfg_for_startup.seeding.backend
                )));
            }
            let refs = active
                .iter()
                .map(|adapter| adapter.as_ref() as &dyn agent::AgentAdapter)
                .collect::<Vec<_>>();
            let _caps = probe_and_persist(
                &refs,
                runtime.process_runner.as_ref(),
                runtime.file_system.as_ref(),
                &startup.scope.working_dir,
            )?;
        }
        if !cfg_for_startup.execution.test_mode {
            let _ = run_startup_audits(runtime, &mut cfg_for_startup, &startup.scope, false)?;
        }
        if cfg_for_startup.execution.worker_mode == "stub_complete" {
            let db_path = startup
                .scope
                .repo_root
                .as_ref()
                .unwrap_or(&startup.scope.working_dir)
                .join(".cache/gardener/backlog.sqlite");
            let store = BacklogStore::open(db_path)?;
            let summary = run_worker_pool_stub(
                &store,
                cfg_for_startup.orchestrator.parallelism as usize,
                target as usize,
            )?;
            runtime.terminal.write_line(&format!(
                "phase4 scheduler-stub complete: target={} completed={}",
                target, summary.completed_count
            ))?;
            return Ok(0);
        }
        let completed = run_worker_pool_fsm(&cfg_for_startup, target as usize, cli.task.as_deref())?;
        runtime.terminal.write_line(&format!(
            "phase5 worker-fsm complete: target={} completed={}",
            target, completed
        ))?;
        return Ok(0);
    }

    let _profile = ensure_profile_for_run(
        runtime,
        &startup.scope,
        &cfg,
        &env_map,
        false,
        cli.agent.map(Into::into),
    )?;

    runtime
        .terminal
        .write_line("phase1 runtime skeleton initialized")?;

    Ok(0)
}

pub fn render_help() -> String {
    let mut cmd = Cli::command();
    let mut buffer = Vec::new();
    cmd.write_long_help(&mut buffer).expect("write help to vec");
    String::from_utf8(buffer).expect("utf8")
}

fn env_to_map(env: &[(std::ffi::OsString, std::ffi::OsString)]) -> EnvMap {
    let mut map = EnvMap::new();
    for (key, value) in env {
        if let (Some(key), Some(value)) = (key.to_str(), value.to_str()) {
            map.insert(key.to_string(), value.to_string());
        }
    }
    map
}

fn persist_agent_default(
    fs: &dyn runtime::FileSystem,
    path: &std::path::Path,
    agent: AgentKind,
) -> Result<(), GardenerError> {
    let existing = fs.read_to_string(path)?;
    let mut value: toml::Value =
        toml::from_str(&existing).map_err(|e| GardenerError::ConfigParse(e.to_string()))?;
    if !value.is_table() {
        return Err(GardenerError::ConfigParse(
            "config root must be table".to_string(),
        ));
    }

    let table = value
        .as_table_mut()
        .ok_or_else(|| GardenerError::ConfigParse("config root must be table".to_string()))?;
    let agent_table = table
        .entry("agent")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let agent_table = agent_table
        .as_table_mut()
        .ok_or_else(|| GardenerError::ConfigParse("agent table invalid".to_string()))?;
    agent_table.insert(
        "default".to_string(),
        toml::Value::String(agent.as_str().to_string()),
    );

    let output =
        toml::to_string_pretty(&value).map_err(|e| GardenerError::ConfigParse(e.to_string()))?;
    fs.write_string(path, &output)
}
