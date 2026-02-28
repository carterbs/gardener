#![deny(clippy::redundant_clone)]

pub mod agent;
pub mod backlog_snapshot;
pub mod backlog_store;
pub mod config;
pub mod errors;
pub mod fsm;
pub mod gh;
pub mod git;
pub mod hotkeys;
pub mod learning_loop;
pub mod log_retention;
pub mod logging;
pub mod output_envelope;
pub mod postmerge_analysis;
pub mod postmortem;
pub mod pr_audit;
pub mod priority;
pub mod prompt_context;
pub mod prompt_knowledge;
pub mod prompt_registry;
pub mod prompts;
pub mod protocol;
pub mod quality_domain_catalog;
pub mod quality_evidence;
pub mod quality_grades;
pub mod quality_scoring;
pub mod repo_intelligence;
pub mod runtime;
pub mod seed_runner;
pub mod seeding;
pub mod startup;
pub mod task_identity;
pub mod triage;
pub mod triage_agent_detection;
pub mod triage_discovery;
pub mod triage_interview;
pub mod tui;
pub mod types;
pub mod worker;
pub mod worker_identity;
pub mod worker_pool;
pub mod worktree;
pub mod worktree_audit;

use agent::factory::AdapterFactory;
use agent::{probe_and_persist, validate_model};
use backlog_snapshot::export_markdown_snapshot;
use backlog_store::BacklogStore;
use clap::{error::ErrorKind, CommandFactory, Parser, ValueEnum};
use config::{load_config, resolve_validation_command, CliOverrides, MergeCompletionMode};
use errors::GardenerError;
use logging::{
    append_run_log, clear_run_logger, default_run_log_path, init_run_logger, set_run_working_dir,
    structured_fallback_line,
};
use runtime::{clear_interrupt, ProcessRequest, ProductionRuntime};
use serde_json::json;
use startup::{backlog_db_path, run_startup_audits, run_startup_audits_with_progress};
use triage::{ensure_profile_for_run, triage_needed, TriageDecision};
use triage_agent_detection::{is_non_interactive, EnvMap};
use tui::{BacklogView, QueueStats, WorkerRow};
use types::{AgentKind, RuntimeScope, ValidationCommandResolution};
use worker_pool::run_worker_pool_fsm;

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
    #[arg(long = "quit-after")]
    pub target: Option<u32>,
    #[arg(long, default_value_t = false)]
    pub prune_only: bool,
    #[arg(long, default_value_t = false)]
    pub backlog_only: bool,
    #[arg(long, default_value_t = false)]
    pub quality_grades_only: bool,
    #[arg(long)]
    pub validation_command: Option<String>,
    #[arg(long, default_value_t = false)]
    pub validate: bool,
    #[arg(long, value_enum)]
    pub agent: Option<CliAgent>,
    #[arg(long, default_value_t = false)]
    pub retriage: bool,
    #[arg(long, default_value_t = false)]
    pub triage_only: bool,
    #[arg(long, default_value_t = false)]
    pub sync_only: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliAgent {
    Claude,
    Codex,
}

fn parse_merge_mode(value: Option<&str>) -> Option<MergeCompletionMode> {
    value.map(|value| match value {
        "local" => MergeCompletionMode::Local,
        "pr_per_completion" | "pr-per-completion" => MergeCompletionMode::PrPerCompletion,
        _ => unreachable!("merge mode was validated by clap argument parser"),
    })
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
    append_run_log(
        "debug",
        "runtime.run.requested",
        json!({
            "invoked_at": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default(),
        }),
    );
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
    clear_interrupt();
    let run_log_path = default_run_log_path(cwd);
    let run_id = init_run_logger(&run_log_path, cwd);
    let _run_log_guard = RunLogGuard;
    append_run_log(
        "info",
        "run.started",
        json!({
            "run_id": run_id,
            "log_path": run_log_path.display().to_string(),
            "cwd": cwd.display().to_string(),
            "arg_count": args.len()
        }),
    );
    let result = (|| -> Result<i32, GardenerError> {
        let _ui_guard = UiGuard::new(runtime.terminal.as_ref());
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
        append_run_log(
            "info",
            "cli.parsed",
            json!({
                "config_override": cli.config.as_ref().map(|p| p.display().to_string()),
                "validate": cli.validate,
                "task_override": cli.task,
                "target": cli.target,
                "triage_only": cli.triage_only,
                "sync_only": cli.sync_only
            }),
        );

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
            sync_only: cli.sync_only,
        };

        let (cfg, scope) = load_config(
            &overrides,
            cwd,
            runtime.file_system.as_ref(),
            runtime.process_runner.as_ref(),
        )?;
        set_run_working_dir(&scope.working_dir);
        append_run_log(
            "info",
            "config.loaded",
            json!({
                "working_dir": scope.working_dir.display().to_string(),
                "repo_root": scope.repo_root.as_ref().map(|p| p.display().to_string()),
                "parallelism": cfg.orchestrator.parallelism
            }),
        );

        if let (Some(agent), Some(config_path)) = (cli.agent, cli.config.as_ref()) {
            persist_agent_default(
                runtime.file_system.as_ref(),
                config_path.as_path(),
                AgentKind::from(agent),
            )?;
        }

        let validation = resolve_validation_command(&cfg, cli.validation_command.as_deref());
        let startup = StartupSnapshot { scope, validation };

        if cli.validate {
            append_run_log(
                "info",
                "cli.validate.started",
                json!({ "command": startup.validation.command }),
            );
            let out = runtime.process_runner.run(ProcessRequest {
                program: "sh".to_string(),
                args: vec!["-lc".to_string(), startup.validation.command.clone()],
                cwd: Some(startup.scope.working_dir.clone()),
            })?;
            append_run_log(
                "info",
                "cli.validate.completed",
                json!({
                    "command": startup.validation.command,
                    "exit_code": out.exit_code,
                }),
            );
            if out.exit_code != 0 {
                return Ok(out.exit_code);
            }
            runtime.terminal.write_line("validation command passed")?;
            return Ok(0);
        }

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
            runtime.terminal.write_line("phase3 backlog-only")?;
            let mut cfg_for_startup = cfg;
            let _ = run_startup_audits(runtime, &mut cfg_for_startup, &startup.scope, true)?;
            return Ok(0);
        }

        if cli.quality_grades_only {
            runtime.terminal.write_line("phase3 quality-grades-only")?;
            let mut cfg_for_startup = cfg;
            let _ = run_startup_audits(runtime, &mut cfg_for_startup, &startup.scope, false)?;
            return Ok(0);
        }

        if cli.sync_only {
            let mut cfg_for_startup = cfg;
            if !cfg_for_startup.execution.test_mode {
                let _ = run_startup_audits(runtime, &mut cfg_for_startup, &startup.scope, false)?;
            }
            let db_path = backlog_db_path(&cfg_for_startup, &startup.scope);
            let snapshot_path = startup
                .scope
                .working_dir
                .join(".cache/gardener/backlog-snapshot.md");
            if let Some(parent) = snapshot_path.parent() {
                runtime.file_system.create_dir_all(parent)?;
            }
            let store = BacklogStore::open(db_path)?;
            let _ = export_markdown_snapshot(&store, &snapshot_path)?;
            runtime.terminal.write_line(&format!(
                "sync complete: snapshot={}",
                snapshot_path.display()
            ))?;
            return Ok(0);
        }

        let default_quit_after = if cli.target.is_none()
            && !cli.prune_only
            && !cli.backlog_only
            && !cli.quality_grades_only
            && !cli.sync_only
            && !cli.triage_only
            && !cli.retriage
        {
            Some(1)
        } else {
            None
        };

        if let Some(target) = cli.target.or(default_quit_after) {
            let mut cfg_for_startup = cfg;
            draw_boot_stage(
                runtime,
                "INIT",
                "Starting Gardener runtime and loading orchestrator state",
            )?;

            let triage_state = triage_needed(&startup.scope, &cfg_for_startup, runtime, false)?;
            match triage_state {
                TriageDecision::Needed => draw_boot_stage(
                    runtime,
                    "TRIAGE",
                    "Collecting repository intelligence and validating setup",
                )?,
                TriageDecision::NotNeeded => draw_boot_stage(
                    runtime,
                    "CHECK_TRIAGE",
                    "Existing repository intelligence is valid",
                )?,
            }
            if !cfg_for_startup.execution.test_mode {
                let profile = ensure_profile_for_run(
                    runtime,
                    &startup.scope,
                    &cfg_for_startup,
                    &env_map,
                    false,
                    cli.agent.map(Into::into),
                )?;
                apply_profile_runtime_preferences(
                    &mut cfg_for_startup,
                    profile.as_ref(),
                    cli.parallelism,
                );
            }
            draw_boot_stage(
                runtime,
                "STARTUP_AUDITS",
                "Refreshing quality grades, worktree/PR health, and startup checks",
            )?;
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
                    runtime.clock.as_ref(),
                    &startup.scope.working_dir,
                )?;
            }
            draw_boot_stage(
                runtime,
                "BACKLOG_SYNC",
                "Seeding and reconciling backlog before worker assignment",
            )?;
            if !cfg_for_startup.execution.test_mode {
                let _ = run_startup_audits_with_progress(
                    runtime,
                    &mut cfg_for_startup,
                    &startup.scope,
                    true,
                    |detail| draw_boot_stage(runtime, "BACKLOG_SYNC", detail),
                )?;
            }
            let db_path = backlog_db_path(&cfg_for_startup, &startup.scope);
            let store = BacklogStore::open(db_path)?;
            let startup_backlog = store.list_tasks()?;
            let startup_backlog_tasks = startup_backlog
                .into_iter()
                .map(|task| {
                    json!({
                        "task_id": task.task_id,
                        "status": task.status.as_str()
                    })
                })
                .collect::<Vec<_>>();
            append_run_log(
                "debug",
                "backlog.startup.snapshot",
                json!({
                    "count": startup_backlog_tasks.len(),
                    "tasks": startup_backlog_tasks,
                }),
            );
            draw_boot_stage(
                runtime,
                "WORKING",
                "Dispatching tasks to workers and streaming progress",
            )?;
            let completed = run_worker_pool_fsm(
                runtime,
                &startup.scope,
                &cfg_for_startup,
                &store,
                runtime.terminal.as_ref(),
                target as usize,
                cli.task.as_deref(),
            )?;
            if !runtime.terminal.stdin_is_tty() {
                runtime.terminal.write_line(&structured_fallback_line(
                    "pool",
                    "complete",
                    &format!("target={target} completed={completed}"),
                ))?;
            }
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
    })();

    match &result {
        Ok(code) => append_run_log("info", "run.completed", json!({ "exit_code": code })),
        Err(error) => append_run_log("error", "run.failed", json!({ "error": error.to_string() })),
    }
    result
}

fn draw_boot_stage(
    runtime: &ProductionRuntime,
    stage: &str,
    detail: &str,
) -> Result<(), GardenerError> {
    append_run_log(
        "info",
        "boot.stage",
        json!({
            "stage": stage,
            "detail": detail
        }),
    );
    if !runtime.terminal.stdin_is_tty() {
        return Ok(());
    }

    let workers = vec![WorkerRow {
        worker_id: "sys".to_string(),
        state: stage.to_ascii_lowercase(),
        task_title: detail.to_string(),
        tool_line: "orchestrator".to_string(),
        breadcrumb: format!("boot>{}", stage.to_ascii_lowercase()),
        last_heartbeat_secs: 0,
        session_age_secs: 0,
        lease_held: false,
        session_missing: false,
        command_details: Vec::new(),
        commands_expanded: false,
    }];
    let stats = QueueStats {
        ready: 0,
        active: 0,
        failed: 0,
        p0: 0,
        p1: 0,
        p2: 0,
    };
    let backlog = BacklogView {
        in_progress: vec![format!("INP SYS {stage}")],
        queued: vec![],
    };
    runtime.terminal.draw_dashboard(&workers, &stats, &backlog)
}

struct UiGuard<'a> {
    terminal: &'a dyn runtime::Terminal,
}

struct RunLogGuard;

impl<'a> UiGuard<'a> {
    fn new(terminal: &'a dyn runtime::Terminal) -> Self {
        Self { terminal }
    }
}

impl Drop for RunLogGuard {
    fn drop(&mut self) {
        clear_run_logger();
    }
}

impl Drop for UiGuard<'_> {
    fn drop(&mut self) {
        let _ = self.terminal.close_ui();
    }
}

fn apply_profile_runtime_preferences(
    cfg: &mut config::AppConfig,
    profile: Option<&repo_intelligence::RepoIntelligenceProfile>,
    cli_parallelism: Option<u32>,
) {
    if cli_parallelism.is_some() {
        return;
    }
    let Some(profile) = profile else {
        return;
    };
    if let Some(parallelism) = profile.user_validated.preferred_parallelism {
        if parallelism > 0 {
            cfg.orchestrator.parallelism = parallelism;
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{config, repo_intelligence, runtime, triage_discovery};
    use std::path::Path;

    fn sample_profile(preferred_parallelism: Option<u32>) -> repo_intelligence::RepoIntelligenceProfile {
        let clock = runtime::FakeClock::default();
        let mut profile = repo_intelligence::build_profile(repo_intelligence::BuildProfileInput {
            clock: &clock,
            working_dir: Path::new("/tmp"),
            repo_root: Path::new("/tmp"),
            head_sha: "deadbeef".to_string(),
            discovery: triage_discovery::DiscoveryAssessment::unknown(),
            discovery_used: false,
            primary_agent: None,
            claude_signals: Vec::new(),
            codex_signals: Vec::new(),
            validation_command: "npm run validate".to_string(),
            agents_md_present: false,
        });
        profile.user_validated.preferred_parallelism = preferred_parallelism;
        profile
    }

    #[test]
    fn profile_parallelism_does_not_override_cli_parallelism() {
        let mut cfg = config::AppConfig::default();
        cfg.orchestrator.parallelism = 4;
        let profile = sample_profile(Some(8));

        super::apply_profile_runtime_preferences(&mut cfg, Some(&profile), Some(4));

        assert_eq!(cfg.orchestrator.parallelism, 4);
    }
}
