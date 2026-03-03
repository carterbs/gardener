#![deny(clippy::manual_strip, clippy::needless_update, clippy::redundant_clone)]

pub mod agent;
pub mod agent_turn;
pub mod backlog_snapshot;
pub mod backlog_store;
pub mod config;
pub mod do_phase;
pub mod errors;
pub mod friction_analysis;
pub mod fsm;
pub mod gh;
pub mod git;
pub mod git_phase;
pub mod hotkeys;
pub mod learning_loop;
pub mod log_query;
pub mod log_retention;
pub mod logging;
pub mod merge_loop;
pub mod output_envelope;
pub mod phase_cli;
pub mod plan_phase;
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
pub mod review_phase;
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
pub mod understand_phase;
pub mod worker;
pub mod worker_identity;
pub mod worker_pool;
pub mod worktree;
pub mod worktree_audit;

use agent::factory::AdapterFactory;
use agent::{probe_and_persist, validate_model};
use backlog_snapshot::export_markdown_snapshot;
use backlog_store::{system_time_unix, BacklogStore, TaskStatus};
use clap::{error::ErrorKind, CommandFactory, Parser, ValueEnum};
use config::{effective_agent_for_state, load_config, resolve_validation_command, CliOverrides};
use errors::GardenerError;
use gh::GhClient;
use logging::{
    append_run_log, clear_run_logger, current_run_id, current_run_log_path, default_run_log_path,
    init_run_logger, set_run_working_dir, structured_fallback_line,
};
use runtime::{clear_interrupt, ProcessRequest, ProductionRuntime};
use serde_json::json;
use startup::{
    backlog_db_path, run_interactive_seeding, run_startup_audits, run_startup_audits_with_progress,
};
use std::collections::{BTreeSet, HashMap};
use triage::{ensure_profile_for_run, triage_needed, TriageDecision};
use triage_agent_detection::{is_non_interactive, EnvMap};
use tui::{BacklogView, QueueStats, WorkerRow};
use types::{AgentKind, RuntimeScope, ValidationCommandResolution, WorkerState};
use worker::worktree_branch_for;
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
    pub num_workers: Option<u32>,
    #[arg(long = "worker-count", conflicts_with = "num_workers", help = "Deprecated: use --num-workers")]
    pub worker_count: Option<u32>,
    #[arg(long)]
    pub task: Option<String>,
    #[arg(long = "quit-after")]
    pub target: Option<u32>,
    #[arg(long, value_enum)]
    pub worker_mode: Option<CliWorkerMode>,
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
    #[arg(long, default_value_t = false)]
    pub force_seed_backlog: bool,
    #[arg(long, default_value_t = false)]
    pub seed_dry_run: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliAgent {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliWorkerMode {
    #[value(name = "normal")]
    Normal,
    #[value(name = "stub_complete")]
    StubComplete,
}

impl CliWorkerMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::StubComplete => "stub_complete",
        }
    }
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
        let cli_parallelism_override = cli.num_workers.or(cli.worker_count);
        append_run_log(
            "info",
            "cli.parsed",
            json!({
                "config_override": cli.config.as_ref().map(|p| p.display().to_string()),
                "validate": cli.validate,
                "task_override": cli.task,
                "target": cli.target,
                "worker_mode": cli.worker_mode.map(|mode| mode.as_str()),
                "triage_only": cli.triage_only,
                "sync_only": cli.sync_only,
                "force_seed_backlog": cli.force_seed_backlog,
                "seed_dry_run": cli.seed_dry_run
            }),
        );
        if cli.worker_count.is_some() {
            let _ = runtime.terminal.write_line(
                "warning: --worker-count is deprecated and will be removed; use --num-workers instead",
            );
        }

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
            parallelism: cli_parallelism_override,
            task: cli.task.clone(),
            target: cli.target,
            worker_mode: cli.worker_mode.map(|mode| mode.as_str().to_string()),
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
            let _ = run_with_startup_capture(runtime, &startup.scope, "backlog-only", || {
                run_startup_audits(
                    runtime,
                    &mut cfg_for_startup,
                    &startup.scope,
                    true,
                    cli.force_seed_backlog,
                    cli.seed_dry_run,
                )
            })?;
            return Ok(0);
        }

        if cli.quality_grades_only {
            runtime.terminal.write_line("phase3 quality-grades-only")?;
            let mut cfg_for_startup = cfg;
            let _ =
                run_with_startup_capture(runtime, &startup.scope, "quality-grades-only", || {
                    run_startup_audits(
                        runtime,
                        &mut cfg_for_startup,
                        &startup.scope,
                        false,
                        false,
                        false,
                    )
                })?;
            return Ok(0);
        }

        if cli.sync_only {
            let mut cfg_for_startup = cfg;
            if !cfg_for_startup.execution.test_mode {
                run_with_startup_capture(runtime, &startup.scope, "sync-only", || {
                    run_startup_audits(
                        runtime,
                        &mut cfg_for_startup,
                        &startup.scope,
                        false,
                        false,
                        false,
                    )
                })?;
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
            store.recover_stale_leases(system_time_unix())?;
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
                    cli_parallelism_override,
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
                for backend in required_agent_backends(&cfg_for_startup)? {
                    if let Some(adapter) = factory.get(backend) {
                        active.push(adapter);
                    } else {
                        return Err(GardenerError::InvalidConfig(format!(
                            "no adapter registered for backend {:?}",
                            backend
                        )));
                    }
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
            let is_tty = runtime.terminal.stdin_is_tty();
            if !cfg_for_startup.execution.test_mode {
                // When TTY, run startup audits without seeding; interactive
                // seeding is handled separately below.
                let run_seeding_in_audits = !is_tty;
                run_with_startup_capture(runtime, &startup.scope, "worker-startup", || {
                    run_startup_audits_with_progress(
                        runtime,
                        &mut cfg_for_startup,
                        &startup.scope,
                        run_seeding_in_audits,
                        cli.force_seed_backlog,
                        cli.seed_dry_run,
                        |detail| draw_boot_stage(runtime, "BACKLOG_SYNC", detail),
                    )
                })?;
            }
            let db_path = backlog_db_path(&cfg_for_startup, &startup.scope);
            let store = BacklogStore::open(db_path)?;
            if is_tty && !cfg_for_startup.execution.test_mode {
                let seeded = run_interactive_seeding(
                    runtime,
                    &startup.scope,
                    &cfg_for_startup,
                    &store,
                    cli.force_seed_backlog,
                )?;
                if seeded > 0 {
                    append_run_log(
                        "info",
                        "startup.interactive_seeding.upserted",
                        json!({ "seeded": seeded }),
                    );
                }
            }
            store.recover_stale_leases(system_time_unix())?;
            let mut startup_backlog = store.list_tasks()?;
            if !cfg_for_startup.execution.test_mode {
                if let Ok(open_prs) = GhClient::new(
                    runtime.process_runner.as_ref(),
                    startup
                        .scope
                        .repo_root
                        .as_ref()
                        .unwrap_or(&startup.scope.working_dir),
                )
                .list_open_prs()
                {
                    let open_pr_map = open_prs
                        .into_iter()
                        .filter(|pr| pr.head_ref_name.starts_with("gardener/"))
                        .map(|pr| (pr.head_ref_name, pr.number))
                        .collect::<HashMap<_, _>>();
                    for task in startup_backlog.iter() {
                        let branch = if let Some(branch) = task.related_branch.as_deref() {
                            branch.to_string()
                        } else {
                            worktree_branch_for(&task.task_id)
                        };
                        if let Some(pr_number) = open_pr_map.get(&branch).copied() {
                            if task.related_pr.is_none() {
                                let _ =
                                    store.set_related_pr(&task.task_id, pr_number as i64, &branch);
                            }
                            if task.status == TaskStatus::Unresolved {
                                let _ = store.set_unresolved_to_merge_pending(&task.task_id);
                            }
                        } else {
                            if task.status == TaskStatus::Unresolved {
                                let _ = store.set_unresolved_to_ready(&task.task_id);
                            }
                            if task.related_pr.is_some() {
                                let _ = store.clear_related_pr(&task.task_id);
                            }
                        }
                    }
                    let _ = store.promote_ready_with_pr();
                } else {
                    append_run_log(
                        "warn",
                        "backlog.startup.open_prs_skipped",
                        json!({ "cwd": startup.scope.working_dir.display().to_string() }),
                    );
                }
                startup_backlog = store.list_tasks()?;
            }
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

fn run_with_startup_capture<T, F>(
    runtime: &ProductionRuntime,
    scope: &crate::types::RuntimeScope,
    stage: &str,
    mut run_startup: F,
) -> Result<T, GardenerError>
where
    F: FnMut() -> Result<T, GardenerError>,
{
    match run_startup() {
        Ok(value) => Ok(value),
        Err(error) => {
            capture_startup_diagnostics(runtime, scope, stage, &error);
            Err(error)
        }
    }
}

fn required_agent_backends(cfg: &config::AppConfig) -> Result<Vec<AgentKind>, GardenerError> {
    append_run_log(
        "debug",
        "agent.prerequisites.resolve.started",
        json!({
            "states": ["understand", "planning", "doing", "gitting", "reviewing", "merging"],
        }),
    );
    let mut adapters = BTreeSet::new();
    let worker_states = [
        WorkerState::Understand,
        WorkerState::Planning,
        WorkerState::Doing,
        WorkerState::Gitting,
        WorkerState::Reviewing,
        WorkerState::Merging,
    ];
    for state in worker_states {
        let backend = effective_agent_for_state(cfg, state).ok_or_else(|| {
            GardenerError::InvalidConfig(format!("no backend configured for {state:?}"))
        })?;
        adapters.insert(backend);
    }
    adapters.insert(cfg.seeding.backend);
    Ok(adapters.into_iter().collect())
}

fn capture_startup_diagnostics(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    stage: &str,
    error: &GardenerError,
) {
    let run_id = current_run_id().unwrap_or_else(|| "unknown".to_string());
    let script_path = scope
        .repo_root
        .as_ref()
        .unwrap_or(&scope.working_dir)
        .join("scripts/startup-diagnostics.sh")
        .display()
        .to_string();

    let mut args = vec![
        script_path,
        "--stage".to_string(),
        stage.to_string(),
        "--run-id".to_string(),
        run_id.clone(),
    ];
    let log_path =
        current_run_log_path().unwrap_or_else(|| default_run_log_path(&scope.working_dir));
    args.push("--log-path".to_string());
    args.push(log_path.display().to_string());
    args.push("--error".to_string());
    args.push(error.to_string());

    append_run_log(
        "warn",
        "startup.diagnostics.capture.requested",
        json!({
            "stage": stage,
            "run_id": run_id,
            "script": args.first().cloned().unwrap_or_default()
        }),
    );

    let request = ProcessRequest {
        program: "bash".to_string(),
        args,
        cwd: Some(scope.working_dir.clone()),
    };

    match runtime.process_runner.run(request) {
        Ok(output) => {
            append_run_log(
                "info",
                "startup.diagnostics.capture.completed",
                json!({
                    "stage": stage,
                    "exit_code": output.exit_code,
                    "stdout_size": output.stdout.len(),
                    "stderr_size": output.stderr.len(),
                }),
            );
        }
        Err(error) => {
            append_run_log(
                "warn",
                "startup.diagnostics.capture.failed",
                json!({
                    "stage": stage,
                    "error": error.to_string(),
                }),
            );
        }
    }
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
        task_id: None,
        last_state_line: 0,
        task_title: detail.to_string(),
        tool_line: "orchestrator".to_string(),
        breadcrumb: format!("boot>{}", stage.to_ascii_lowercase()),
        last_heartbeat_secs: 0,
        session_age_secs: 0,
        lease_held: false,
        session_missing: false,
        command_details: Vec::new(),
    }];
    let stats = QueueStats {
        ready: 0,
        active: 0,
        failed: 0,
        unresolved: 0,
        merge_pending: 0,
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
    if let Err(err) = cmd.write_long_help(&mut buffer) {
        panic!("write help to vec: {err}");
    }
    match String::from_utf8(buffer) {
        Ok(result) => result,
        Err(err) => panic!("utf8: {err}"),
    }
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
    use clap::{error::ErrorKind, Parser};
    use std::path::Path;

    fn sample_profile(
        preferred_parallelism: Option<u32>,
    ) -> repo_intelligence::RepoIntelligenceProfile {
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

    #[test]
    fn parse_num_workers_cli_flag() {
        let cli = super::Cli::try_parse_from(["gardener", "--num-workers", "5"])
            .expect("new flag should parse");
        assert_eq!(cli.num_workers, Some(5));
        assert_eq!(cli.worker_count, None);
    }

    #[test]
    fn parse_worker_count_alias_cli_flag() {
        let cli = super::Cli::try_parse_from(["gardener", "--worker-count", "5"])
            .expect("deprecated alias should parse");
        assert_eq!(cli.worker_count, Some(5));
        assert_eq!(cli.num_workers, None);
    }

    #[test]
    fn parse_conflicting_worker_count_and_num_workers_is_rejected() {
        let err = super::Cli::try_parse_from([
            "gardener",
            "--num-workers",
            "5",
            "--worker-count",
            "6",
        ])
        .expect_err("conflicting worker count flags should fail");
        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn record_session_flag_is_removed() {
        let parse =
            super::Cli::try_parse_from(["gardener", "--record-session", "tmp/session.jsonl"]);
        assert!(parse.is_err());
    }

    #[test]
    fn required_agent_backends_collects_unique_backends() {
        let mut cfg = config::AppConfig::default();
        cfg.states.insert(
            "understand".to_string(),
            config::StateConfig {
                backend: Some(crate::types::AgentKind::Claude),
                model: None,
            },
        );

        let backends = super::required_agent_backends(&cfg).unwrap_or_default();
        assert_eq!(
            backends,
            vec![
                crate::types::AgentKind::Claude,
                crate::types::AgentKind::Codex
            ]
        );
    }

    #[test]
    fn required_agent_backends_requires_effective_backend_for_each_active_state() {
        let mut cfg = config::AppConfig::default();
        cfg.agent.default = None;
        cfg.states.insert(
            "planning".to_string(),
            config::StateConfig {
                backend: Some(crate::types::AgentKind::Claude),
                model: None,
            },
        );
        cfg.states.insert(
            "doing".to_string(),
            config::StateConfig {
                backend: None,
                model: None,
            },
        );

        let error = super::required_agent_backends(&cfg).expect_err("expected missing backend");
        assert!(error.to_string().contains("no backend configured for"));
    }
}
