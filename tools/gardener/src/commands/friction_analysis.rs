#![deny(
    clippy::manual_strip,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::redundant_clone
)]

use gardener::backlog_store::system_time_unix;
use gardener::config::load_config;
use gardener::config::CliOverrides;
use gardener::friction_analysis::{
    extract_worker_timeline, findings_to_tasks, run_friction_analysis, FrictionAnalysisInput,
    FrictionAnalysisOutcome,
};
use gardener::logging::append_run_log;
use gardener::runtime::ProductionRuntime;
use gardener::startup::backlog_db_path;
use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "friction-analysis")]
#[command(about = "Run post-merge friction analysis on OTEL logs")]
struct Args {
    /// Path to the OTEL JSONL log file
    #[arg(long)]
    log_path: PathBuf,

    /// Run ID to filter events
    #[arg(long)]
    run_id: String,

    /// Worker ID to filter events
    #[arg(long)]
    worker_id: String,

    /// Task ID (for context in the analysis prompt)
    #[arg(long, default_value = "unknown")]
    task_id: String,

    /// Task summary (for context in the analysis prompt)
    #[arg(long, default_value = "unknown task")]
    task_summary: String,

    /// Merge SHA (for context in the analysis prompt)
    #[arg(long)]
    merge_sha: Option<String>,

    /// Only extract and print the timeline, don't run LLM analysis
    #[arg(long, default_value_t = false)]
    timeline_only: bool,

    /// Write findings to the backlog database
    #[arg(long, default_value_t = false)]
    write_backlog: bool,

    /// Path to gardener config file (optional, uses defaults if not provided)
    #[arg(long)]
    config: Option<PathBuf>,
}

pub fn run_with_args(args: &[String]) -> Result<i32, gardener::errors::GardenerError> {
    let args = Args::parse_from(args);

    // Step 1: Extract timeline
    eprintln!(
        "Extracting timeline for run={} worker={} from {}",
        args.run_id,
        args.worker_id,
        args.log_path.display()
    );

    let timeline = extract_worker_timeline(&args.log_path, &args.run_id, &args.worker_id)?;

    if timeline.is_empty() {
        eprintln!("No matching events found.");
        return Ok(0);
    }

    eprintln!(
        "Timeline: {} bytes, {} events",
        timeline.len(),
        timeline.lines().count()
    );
    append_run_log(
        "info",
        "friction_analysis.timeline.extracted",
        serde_json::json!({
            "run_id": args.run_id,
            "worker_id": args.worker_id,
            "bytes": timeline.len(),
            "events": timeline.lines().count()
        }),
    );

    if args.timeline_only {
        println!("{timeline}");
        return Ok(0);
    }

    // Step 2: Load config for LLM access
    let cwd =
        std::env::current_dir().map_err(|e| gardener::errors::GardenerError::Io(e.to_string()))?;
    let runtime = ProductionRuntime::new();
    let overrides = CliOverrides {
        config_path: args.config.clone(),
        working_dir: None,
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

    let (cfg, scope) = load_config(
        &overrides,
        &cwd,
        runtime.file_system.as_ref(),
        runtime.process_runner.as_ref(),
    )?;

    eprintln!(
        "Using backend={:?} model={}",
        cfg.seeding.backend, cfg.seeding.model
    );

    // Step 3: Run friction analysis
    let input = FrictionAnalysisInput {
        worker_id: &args.worker_id,
        task_id: &args.task_id,
        task_summary: &args.task_summary,
        merge_sha: args.merge_sha.as_deref(),
        run_id: &args.run_id,
        log_path: &args.log_path,
    };

    match run_friction_analysis(&input, &cfg, runtime.process_runner.as_ref(), &scope)? {
        FrictionAnalysisOutcome::Completed {
            findings,
            smooth_run,
        } => {
            if findings.is_empty() {
                eprintln!("No friction findings.");
                println!(
                    "{{\"findings\": [], \"smooth_run\": {}}}",
                    if smooth_run { "true" } else { "false" }
                );
                return Ok(0);
            }

            eprintln!("Found {} friction finding(s):", findings.len());
            for (i, f) in findings.iter().enumerate() {
                eprintln!("  {}. [{}] {} ({})", i + 1, f.severity, f.title, f.category);
            }

            // Print full JSON to stdout
            let output = serde_json::json!({
                "findings": findings,
                "smooth_run": false
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{{}}".to_string())
            );

            // Optionally write to backlog
            if args.write_backlog {
                let db_path = backlog_db_path(&cfg, &scope);
                eprintln!("Writing to backlog at {}", db_path.display());
                let store = gardener::backlog_store::BacklogStore::open(db_path)?;
                store.recover_stale_leases(system_time_unix())?;
                let tasks = findings_to_tasks(&findings);
                for task in &tasks {
                    match store.upsert_task(task.clone()) {
                        Ok(bt) => eprintln!("  Created task: {} ({})", bt.title, bt.task_id),
                        Err(e) => eprintln!("  Failed to create task: {e}"),
                    }
                }
                eprintln!("Wrote {} task(s) to backlog.", tasks.len());
            }
        }
        FrictionAnalysisOutcome::Skipped { reason } => {
            eprintln!("Skipped: {reason}");
        }
    }

    Ok(0)
}
