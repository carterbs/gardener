#![deny(
    clippy::manual_strip,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::redundant_clone
)]

use clap::{Args, Parser, Subcommand};
use gardener::errors::GardenerError;
use gardener::log_query::{
    discover_log_files, filter_records, format_time, parse_time_filter,
    run_trace as build_run_trace, FilterOptions, LogRecord,
};
use gardener::logging::default_run_log_path;
use gardener::logging::structured_fallback_line;
use serde::Serialize;
use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "otel-logs")]
#[command(about = "Query Gardener OTEL JSONL logs")]
struct Cli {
    /// Path to base OTEL log file (defaults to current working directory policy)
    #[arg(long, global = true)]
    log_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Show metadata for rotated log files.
    Index(IndexArgs),

    /// Filter matching events across rotated log files.
    Filter(FilterArgs),

    /// Build a high-signal lifecycle trace for a run.
    RunTrace(RunTraceArgs),
}

#[derive(Debug, Args)]
struct IndexArgs {
    /// Print machine-readable JSON output.
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Debug, Args)]
struct FilterArgs {
    /// Filter to a specific run id.
    #[arg(long)]
    run_id: Option<String>,

    /// Filter to a specific worker id.
    #[arg(long)]
    worker_id: Option<String>,

    /// Filter to event_type starting with this prefix.
    #[arg(long)]
    event_type: Option<String>,

    /// Exclude events before this time (RFC3339 or unix nanos).
    #[arg(long)]
    since: Option<String>,

    /// Exclude events after this time (RFC3339 or unix nanos).
    #[arg(long)]
    until: Option<String>,

    /// Maximum number of matching records.
    #[arg(long, default_value_t = 500)]
    max: usize,

    /// Return the last N matches instead of first N.
    #[arg(long, default_value_t = false)]
    tail: bool,

    /// Restrict scanning to the most recent N files.
    #[arg(long)]
    files: Option<usize>,

    /// Print matching records as a JSON array.
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Debug, Args)]
struct RunTraceArgs {
    /// Run id to trace.
    #[arg(long)]
    run_id: String,
}

#[derive(Debug, Serialize)]
struct FilterEvent {
    source_file: String,
    line_number: usize,
    time_rfc3339: String,
    event_type: String,
    run_id: String,
    worker_id: String,
    payload: serde_json::Value,
}

fn main() {
    let _ = structured_fallback_line("otel_logs", "main", "started");
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("otel_logs", "run", "dispatching");
    let args = Cli::parse();
    let log_path = resolve_log_path(args.log_path)?;

    match args.command {
        Commands::Index(args) => run_index(&log_path, args),
        Commands::Filter(args) => run_filter(&log_path, args),
        Commands::RunTrace(args) => run_run_trace(&log_path, args),
    }
}

fn resolve_log_path(custom: Option<PathBuf>) -> Result<PathBuf, GardenerError> {
    let _ = structured_fallback_line("otel_logs", "resolve_log_path", "resolving");
    if let Some(path) = custom {
        return Ok(path);
    }

    let cwd = std::env::current_dir().map_err(|error| GardenerError::Io(error.to_string()))?;
    Ok(default_run_log_path(&cwd))
}

fn parse_time_or_error(name: &str, value: &Option<String>) -> Result<Option<u64>, GardenerError> {
    value
        .as_deref()
        .map(|raw| {
            parse_time_filter(raw).ok_or_else(|| {
                GardenerError::InvalidConfig(format!("invalid {name} value {raw:?}"))
            })
        })
        .transpose()
}

fn run_index(log_path: &std::path::Path, args: IndexArgs) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("otel_logs", "run_index", "started");
    let indexes: Vec<_> = discover_log_files(log_path)
        .into_iter()
        .map(|path| gardener::log_query::index_file(&path))
        .collect();

    if args.json {
        let payload = serde_json::to_string_pretty(&indexes).unwrap_or_else(|_| "[]".to_string());
        println!("{payload}");
        return Ok(0);
    }

    println!("FILE\tSIZE\tLINES\tFROM\tTO\tRUNS\tWORKERS");
    for index in indexes {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{} run(s)\t{} worker(s)",
            index.path.display(),
            index.size_bytes,
            index.line_count,
            format_time(index.first_time_nano),
            format_time(index.last_time_nano),
            index.run_ids.len(),
            index.worker_ids.len(),
        );
    }

    Ok(0)
}

fn run_filter(log_path: &std::path::Path, args: FilterArgs) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("otel_logs", "run_filter", "started");
    let since = parse_time_or_error("--since", &args.since)?;
    let until = parse_time_or_error("--until", &args.until)?;

    if let (Some(since), Some(until)) = (since, until) {
        if until < since {
            return Err(GardenerError::InvalidConfig(
                "--until must be greater than or equal to --since".to_string(),
            ));
        }
    }

    let matches = filter_records(
        log_path,
        FilterOptions {
            run_id: args.run_id,
            worker_id: args.worker_id,
            event_type_prefix: args.event_type,
            since,
            until,
        },
        args.files,
    )
    .map_err(|error: io::Error| GardenerError::Io(error.to_string()))?;

    if args.tail {
        run_filter_tail(matches, args.max, args.json)
    } else {
        run_filter_head(matches, args.max, args.json)
    }
}

fn run_filter_head(
    matches: Vec<LogRecord>,
    max: usize,
    output_json: bool,
) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("otel_logs", "run_filter_head", "printing");
    let events = matches
        .into_iter()
        .take(max)
        .map(export_filter_event)
        .collect::<Vec<_>>();

    print_filter_output(&events, output_json)
}

fn run_filter_tail(
    matches: Vec<LogRecord>,
    max: usize,
    output_json: bool,
) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("otel_logs", "run_filter_tail", "printing");
    if max == 0 {
        return Ok(0);
    }

    let mut buffer = VecDeque::new();
    for record in matches {
        if buffer.len() >= max {
            buffer.pop_front();
        }
        buffer.push_back(export_filter_event(record));
    }

    let events = buffer.into_iter().collect::<Vec<_>>();
    print_filter_output(&events, output_json)
}

fn export_filter_event(record: LogRecord) -> FilterEvent {
    let _ = structured_fallback_line("otel_logs", "export_filter_event", "mapping");
    FilterEvent {
        source_file: record.source_file.to_string_lossy().to_string(),
        line_number: record.line_number,
        time_rfc3339: format_time(record.time_unix_nano),
        event_type: record.event_type,
        run_id: record.run_id,
        worker_id: record.worker_id,
        payload: record.payload,
    }
}

fn print_filter_output(events: &[FilterEvent], output_json: bool) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("otel_logs", "print_filter_output", "printing");
    if output_json {
        let payload = serde_json::to_string_pretty(events).unwrap_or_else(|_| "[]".to_string());
        println!("{payload}");
        return Ok(0);
    }

    for event in events {
        let line = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
        println!("{line}");
    }

    Ok(0)
}

fn run_run_trace(log_path: &std::path::Path, args: RunTraceArgs) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("otel_logs", "run_run_trace", "started");
    let trace = build_run_trace(log_path, &args.run_id)
        .map_err(|error: io::Error| GardenerError::Io(error.to_string()))?;

    let Some(trace) = trace else {
        return Err(GardenerError::InvalidConfig(format!(
            "run id not found: {}",
            args.run_id
        )));
    };

    let payload = serde_json::to_string_pretty(&trace).unwrap_or_else(|_| "{}".to_string());
    println!("{payload}");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::parse_time_or_error;
    use super::resolve_log_path;
    use gardener::logging::default_run_log_path;

    #[test]
    fn parse_invalid_time_bound_is_rejected() {
        let error = match parse_time_or_error("--since", &Some("not-a-time".to_string())) {
            Ok(_) => panic!("invalid time should reject"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            gardener::errors::GardenerError::InvalidConfig(_)
        ));
    }

    #[test]
    fn resolve_log_path_defaults_to_expected_path() {
        let cwd = match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => panic!("current_dir should be readable: {error}"),
        };
        let resolved = match resolve_log_path(None) {
            Ok(resolved) => resolved,
            Err(error) => panic!("resolve_log_path should succeed: {error}"),
        };
        assert_eq!(resolved, default_run_log_path(&cwd));
    }
}
