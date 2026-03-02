#![deny(clippy::unwrap_used, clippy::expect_used, clippy::redundant_clone)]

use clap::{Parser, ValueEnum};
use gardener::config::{load_config, CliOverrides};
use gardener::errors::GardenerError;
use gardener::logging::{append_run_log, default_run_log_path, init_run_logger};
use gardener::runtime::ProductionRuntime;
use gardener::startup::run_startup_audits_with_progress;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SeedMode {
    DryRun,
    Write,
}

#[derive(Debug, Parser)]
#[command(name = "seed-backlog")]
#[command(about = "Run Gardener backlog seeding as a standalone phase binary")]
struct Args {
    /// Optional path to config file.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Optional working directory override.
    #[arg(long)]
    working_dir: Option<PathBuf>,

    /// Seeding mode: dry-run prints recommended tasks; write inserts tasks into backlog.
    #[arg(long, value_enum, default_value_t = SeedMode::DryRun)]
    mode: SeedMode,

    /// Force seeding even when active backlog tasks already exist.
    #[arg(long, default_value_t = false)]
    force_seed_backlog: bool,
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, GardenerError> {
    let args = Args::parse();
    let cwd = std::env::current_dir().map_err(|e| GardenerError::Io(e.to_string()))?;
    let log_path = default_run_log_path(&cwd);
    init_run_logger(&log_path, &cwd);
    append_run_log(
        "info",
        "run.started",
        serde_json::json!({
            "log_path": log_path.display().to_string(),
        }),
    );
    append_run_log(
        "info",
        "bin.seed_backlog.started",
        serde_json::json!({
            "config": args.config.as_ref().map(|p| p.display().to_string()),
            "working_dir": args.working_dir.as_ref().map(|p| p.display().to_string()),
            "mode": format!("{:?}", args.mode),
            "force_seed_backlog": args.force_seed_backlog,
        }),
    );
    let runtime = ProductionRuntime::new();

    let overrides = CliOverrides {
        config_path: args.config.clone(),
        working_dir: args.working_dir.clone(),
        parallelism: None,
        task: None,
        target: None,
        prune_only: false,
        backlog_only: false,
        quality_grades_only: false,
        validation_command: None,
        worker_mode: None,
        agent: None,
        retriage: false,
        triage_only: false,
        sync_only: false,
    };

    let (mut cfg, scope) = load_config(
        &overrides,
        &cwd,
        runtime.file_system.as_ref(),
        runtime.process_runner.as_ref(),
    )?;

    let seed_dry_run = matches!(args.mode, SeedMode::DryRun);
    let summary = run_startup_audits_with_progress(
        &runtime,
        &mut cfg,
        &scope,
        true,
        args.force_seed_backlog,
        seed_dry_run,
        |detail| {
            eprintln!("[seed-backlog] {detail}");
            Ok(())
        },
    )?;

    eprintln!(
        "[seed-backlog] complete: quality={} seeded_tasks_upserted={} mode={:?}",
        summary.quality_path.display(),
        summary.seeded_tasks_upserted,
        args.mode
    );
    append_run_log(
        "info",
        "bin.seed_backlog.completed",
        serde_json::json!({
            "quality_path": summary.quality_path.display().to_string(),
            "seeded_tasks_upserted": summary.seeded_tasks_upserted,
            "mode": format!("{:?}", args.mode),
        }),
    );

    Ok(0)
}
