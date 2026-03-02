#![deny(
    clippy::manual_strip,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::redundant_clone
)]

use clap::{Args as ClapArgs, Parser, Subcommand};
use gardener::errors::GardenerError;
use gardener::logging::{default_run_log_path, structured_fallback_line};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::PathBuf;

const TIMELINE_MIN_SEVERITY: u8 = 9;
const TIMELINE_NOISE_PREFIXES: &[&str] = &["boot.stage.", "prompt.rendered"];
const TIMELINE_INTERESTING_PREFIXES: &[&str] =
    &["worker.", "agent.turn.", "merge_loop.", "worker.gitting."];

#[derive(Debug, Parser)]
#[command(name = "log-query")]
#[command(about = "Query Gardener OTEL JSONL logs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to OTEL JSONL log file
    #[arg(long, global = true)]
    log_path: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Print matching events one line at a time
    Events(Events),

    /// Build a compact timeline view from filtered events
    Timeline(Timeline),

    /// Summarize matching events by run, worker, and type
    Stats(Stats),
}

#[derive(Debug, ClapArgs, Clone)]
struct EventFilters {
    /// Filter by run.id attribute
    #[arg(long)]
    run_id: Option<String>,

    /// Filter by worker id
    #[arg(long)]
    worker_id: Option<String>,

    /// Match events by event type substring
    #[arg(long)]
    event_type: Option<String>,

    /// Minimum numeric severity (logRecord.severityNumber)
    #[arg(long)]
    min_severity: Option<u8>,

    /// Search substring within event payload JSON
    #[arg(long)]
    contains: Option<String>,
}

#[derive(Debug, ClapArgs)]
struct Events {
    #[command(flatten)]
    filters: EventFilters,

    /// Maximum number of events to print (0 = all)
    #[arg(long, default_value_t = 0)]
    limit: usize,

    /// Print raw JSON lines instead of compact summaries
    #[arg(long, default_value_t = false)]
    raw: bool,
}

#[derive(Debug, ClapArgs)]
struct Timeline {
    #[command(flatten)]
    filters: EventFilters,

    /// Maximum number of timeline entries (0 = all)
    #[arg(long, default_value_t = 0)]
    limit: usize,
}

#[derive(Debug, ClapArgs)]
struct Stats {
    #[command(flatten)]
    filters: EventFilters,
}

#[derive(Debug, Clone)]
struct LogEvent {
    line_no: usize,
    raw: String,
    event_type: String,
    run_id: String,
    worker_id: String,
    severity_text: String,
    severity_number: u8,
    payload_text: String,
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("log-query", "run", "start");
    let args = Cli::parse();
    let log_path = resolve_log_path(args.log_path)?;
    let events = read_log_events(&log_path)?;

    match args.command {
        Commands::Events(args) => run_events(events, args),
        Commands::Timeline(args) => run_timeline(events, args),
        Commands::Stats(args) => run_stats(events, args),
    }
}

fn run_events(events: Vec<LogEvent>, args: Events) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("log-query", "events", "start");
    let mut printed = 0usize;
    for event in events {
        if !event_matches_filters(&event, &args.filters) {
            continue;
        }
        if args.raw {
            println!("{}", event.raw);
        } else {
            println!("{}", compact_event_line(&event));
        }
        printed += 1;
        if args.limit > 0 && printed >= args.limit {
            break;
        }
    }
    if printed == 0 {
        eprintln!("no events matched filters");
    }
    Ok(0)
}

fn run_timeline(events: Vec<LogEvent>, args: Timeline) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("log-query", "timeline", "start");
    let timeline = timeline_lines(&events, &args.filters);
    if timeline.is_empty() {
        eprintln!("no timeline events matched filters");
        return Ok(0);
    }

    let limit = if args.limit == 0 {
        timeline.len()
    } else {
        args.limit.min(timeline.len())
    };
    let start = timeline.len().saturating_sub(limit);
    for line in timeline.iter().skip(start) {
        println!("{line}");
    }
    Ok(0)
}

fn run_stats(events: Vec<LogEvent>, args: Stats) -> Result<i32, GardenerError> {
    let _ = structured_fallback_line("log-query", "stats", "start");
    let mut total = 0u64;
    let mut by_run: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_worker: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_event_type: BTreeMap<String, u64> = BTreeMap::new();

    for event in events {
        if !event_matches_filters(&event, &args.filters) {
            continue;
        }
        total = total.saturating_add(1);
        let run_key = if event.run_id.is_empty() {
            "<unknown>".to_string()
        } else {
            event.run_id
        };
        let worker_key = if event.worker_id.is_empty() {
            "<unknown>".to_string()
        } else {
            event.worker_id
        };

        by_run
            .entry(run_key)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
        by_worker
            .entry(worker_key)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
        by_event_type
            .entry(event.event_type)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
    }

    let payload = serde_json::json!({
        "total": total,
        "run_ids": by_run,
        "workers": by_worker,
        "event_types": by_event_type,
    });
    let rendered = serde_json::to_string_pretty(&payload)
        .unwrap_or_else(|_| "{\"error\":\"failed to serialize stats\"}".to_string());
    println!("{rendered}");
    Ok(0)
}

fn resolve_log_path(custom: Option<PathBuf>) -> Result<PathBuf, GardenerError> {
    let _ = structured_fallback_line("log-query", "resolve_log_path", "start");
    match custom {
        Some(path) => Ok(path),
        None => {
            let cwd = std::env::current_dir().map_err(|e| GardenerError::Io(e.to_string()))?;
            Ok(default_run_log_path(&cwd))
        }
    }
}

fn read_log_events(path: &std::path::Path) -> Result<Vec<LogEvent>, GardenerError> {
    let _ = structured_fallback_line("log-query", "read_log_events", "start");
    let raw_text = std::fs::read_to_string(path).map_err(|e| {
        GardenerError::Io(format!("failed to read log path {}: {e}", path.display()))
    })?;

    let mut events = Vec::new();
    for (idx, line) in raw_text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(event) = parse_log_event(idx + 1, &value, line) {
            events.push(event);
        }
    }
    Ok(events)
}

fn event_matches_filters(event: &LogEvent, filters: &EventFilters) -> bool {
    if let Some(run_id) = &filters.run_id {
        if event.run_id != run_id.as_str() {
            return false;
        }
    }
    if let Some(worker_id) = &filters.worker_id {
        if event.worker_id != worker_id.as_str() {
            return false;
        }
    }
    if let Some(filter) = &filters.event_type {
        if !event.event_type.contains(filter) {
            return false;
        }
    }
    if let Some(min_severity) = filters.min_severity {
        if event.severity_number < min_severity {
            return false;
        }
    }
    if let Some(contains) = &filters.contains {
        if !event.payload_text.contains(contains) {
            return false;
        }
    }
    true
}

fn parse_log_event(line_no: usize, value: &Value, raw: &str) -> Option<LogEvent> {
    let event_type = extract_event_type(value)?;
    let run_id = extract_run_id(value);
    let worker_id = extract_worker_id(value);
    let (severity_text, severity_number) = extract_severity(value);
    let payload_text = extract_payload(value)
        .as_ref()
        .and_then(|p| serde_json::to_string(p).ok())
        .unwrap_or_default();

    Some(LogEvent {
        line_no,
        raw: raw.to_string(),
        event_type,
        run_id,
        worker_id,
        severity_text,
        severity_number,
        payload_text,
    })
}

fn timeline_lines(events: &[LogEvent], filters: &EventFilters) -> Vec<String> {
    let mut lines = Vec::new();
    for event in events.iter() {
        if !event_matches_filters(event, filters) {
            continue;
        }
        if TIMELINE_NOISE_PREFIXES
            .iter()
            .any(|p| event.event_type.starts_with(p))
        {
            continue;
        }

        let interesting = TIMELINE_INTERESTING_PREFIXES
            .iter()
            .any(|p| event.event_type.starts_with(p));
        if !interesting && event.severity_number < TIMELINE_MIN_SEVERITY {
            continue;
        }
        let payload = if event.payload_text.is_empty() {
            "<empty payload>"
        } else {
            event.payload_text.as_str()
        };
        lines.push(format!(
            "{} {} {}: {}",
            event.severity_text, event.event_type, event.line_no, payload
        ));
    }
    lines
}

fn compact_event_line(event: &LogEvent) -> String {
    let mut out = String::new();
    let worker_label = if event.worker_id.is_empty() {
        "<unknown>"
    } else {
        event.worker_id.as_str()
    };
    let _ = write!(
        &mut out,
        "{} {:5} {} run={} worker={} ",
        event.line_no, event.severity_text, event.event_type, event.run_id, worker_label
    );
    let payload = if event.payload_text.is_empty() {
        "<empty>"
    } else {
        event.payload_text.as_str()
    };
    let _ = write!(&mut out, "{payload}");
    out
}

fn extract_payload(value: &Value) -> Option<Value> {
    if let Some(payload) = value.get("payload") {
        return Some(payload.clone());
    }
    if let Some(attrs) = value
        .pointer("/logRecord/attributes")
        .and_then(Value::as_array)
    {
        if let Some(raw) = extract_attr_value(attrs, "gardener.payload") {
            if let Ok(payload) = serde_json::from_str::<Value>(&raw) {
                return Some(payload);
            }
        }
    }
    None
}

fn extract_run_id(value: &Value) -> String {
    if let Some(attrs) = value
        .pointer("/logRecord/attributes")
        .and_then(Value::as_array)
    {
        if let Some(found) = extract_attr_value(attrs, "run.id") {
            return found;
        }
    }
    value
        .get("payload")
        .and_then(|payload| payload.get("run_id").or_else(|| payload.get("run.id")))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn extract_worker_id(value: &Value) -> String {
    if let Some(payload) = value.get("payload") {
        if let Some(worker_id) = payload.get("worker_id").and_then(Value::as_str) {
            return worker_id.to_string();
        }
    }
    if let Some(attrs) = value
        .pointer("/logRecord/attributes")
        .and_then(Value::as_array)
    {
        if let Some(found) = extract_attr_value(attrs, "payload.worker_id") {
            return found;
        }
        if let Some(raw) = extract_attr_value(attrs, "gardener.payload") {
            if let Ok(payload) = serde_json::from_str::<Value>(&raw) {
                if let Some(worker_id) = payload.get("worker_id").and_then(Value::as_str) {
                    return worker_id.to_string();
                }
            }
        }
    }
    String::new()
}

fn extract_event_type(value: &Value) -> Option<String> {
    if let Some(event_type) = value.get("event_type").and_then(Value::as_str) {
        return Some(event_type.to_string());
    }
    if let Some(attrs) = value
        .pointer("/logRecord/attributes")
        .and_then(Value::as_array)
    {
        if let Some(found) = extract_attr_value(attrs, "event.type") {
            return Some(found);
        }
    }
    if let Some(event_type) = value
        .get("payload")
        .and_then(|payload| payload.get("event_type"))
        .and_then(Value::as_str)
    {
        return Some(event_type.to_string());
    }
    None
}

fn extract_severity(value: &Value) -> (String, u8) {
    let text = value
        .pointer("/logRecord/severityText")
        .and_then(Value::as_str)
        .unwrap_or("INFO");
    let number = value
        .pointer("/logRecord/severityNumber")
        .and_then(Value::as_u64)
        .and_then(|n| u8::try_from(n).ok())
        .unwrap_or(9);
    (text.to_string(), number)
}

fn extract_attr_value(attrs: &[Value], key: &str) -> Option<String> {
    for attr in attrs {
        let matches = attr.get("key").and_then(Value::as_str) == Some(key);
        if !matches {
            continue;
        }
        if let Some(value) = attr
            .get("value")
            .and_then(|value| value.get("stringValue"))
            .and_then(Value::as_str)
        {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn compact_event_line_includes_core_fields() {
        let event = LogEvent {
            line_no: 1,
            raw: r#"{"event_type":"worker.started"}"#.to_string(),
            event_type: "worker.started".to_string(),
            run_id: "run-1".to_string(),
            worker_id: "w1".to_string(),
            severity_text: "INFO".to_string(),
            severity_number: 9,
            payload_text: "{\"worker_id\":\"w1\"}".to_string(),
        };
        let line = compact_event_line(&event);
        assert!(line.contains("1"));
        assert!(line.contains("worker.started"));
        assert!(line.contains("run=run-1"));
    }

    #[test]
    fn timeline_lines_drop_noise() {
        let events = vec![
            LogEvent {
                line_no: 1,
                raw: String::new(),
                event_type: "boot.stage.init".to_string(),
                run_id: String::new(),
                worker_id: "w1".to_string(),
                severity_text: "INFO".to_string(),
                severity_number: 5,
                payload_text: "{}".to_string(),
            },
            LogEvent {
                line_no: 2,
                raw: String::new(),
                event_type: "worker.started".to_string(),
                run_id: String::new(),
                worker_id: "w1".to_string(),
                severity_text: "INFO".to_string(),
                severity_number: 5,
                payload_text: "{}".to_string(),
            },
            LogEvent {
                line_no: 3,
                raw: String::new(),
                event_type: "boring.debug".to_string(),
                run_id: String::new(),
                worker_id: "w1".to_string(),
                severity_text: "DEBUG".to_string(),
                severity_number: 1,
                payload_text: "{}".to_string(),
            },
        ];
        let filters = EventFilters {
            run_id: None,
            worker_id: None,
            event_type: None,
            min_severity: None,
            contains: None,
        };
        let lines = timeline_lines(&events, &filters);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("worker.started"));
    }

    #[test]
    fn event_filter_applies_criteria() {
        let event = LogEvent {
            line_no: 1,
            raw: String::new(),
            event_type: "worker.started".to_string(),
            run_id: "run-1".to_string(),
            worker_id: "worker-1".to_string(),
            severity_text: "INFO".to_string(),
            severity_number: 9,
            payload_text: "{\"key\":\"value\"}".to_string(),
        };
        let filters = EventFilters {
            run_id: Some("run-1".to_string()),
            worker_id: Some("worker-1".to_string()),
            event_type: Some("worker".to_string()),
            min_severity: Some(7),
            contains: Some("key".to_string()),
        };
        assert!(event_matches_filters(&event, &filters));
    }

    #[test]
    fn event_filter_rejects_by_run_id() {
        let event = LogEvent {
            line_no: 1,
            raw: String::new(),
            event_type: "worker.started".to_string(),
            run_id: "run-1".to_string(),
            worker_id: "worker-1".to_string(),
            severity_text: "INFO".to_string(),
            severity_number: 9,
            payload_text: "{}".to_string(),
        };
        let filters = EventFilters {
            run_id: Some("run-2".to_string()),
            worker_id: None,
            event_type: None,
            min_severity: None,
            contains: None,
        };
        assert!(!event_matches_filters(&event, &filters));
    }

    #[test]
    fn event_filter_rejects_by_worker_id() {
        let event = LogEvent {
            line_no: 1,
            raw: String::new(),
            event_type: "worker.started".to_string(),
            run_id: "run-1".to_string(),
            worker_id: "worker-1".to_string(),
            severity_text: "INFO".to_string(),
            severity_number: 9,
            payload_text: "{}".to_string(),
        };
        let filters = EventFilters {
            run_id: None,
            worker_id: Some("worker-2".to_string()),
            event_type: None,
            min_severity: None,
            contains: None,
        };
        assert!(!event_matches_filters(&event, &filters));
    }

    #[test]
    fn event_filter_rejects_low_severity() {
        let event = LogEvent {
            line_no: 1,
            raw: String::new(),
            event_type: "worker.started".to_string(),
            run_id: "run-1".to_string(),
            worker_id: "worker-1".to_string(),
            severity_text: "DEBUG".to_string(),
            severity_number: 4,
            payload_text: "{}".to_string(),
        };
        let filters = EventFilters {
            run_id: None,
            worker_id: None,
            event_type: None,
            min_severity: Some(9),
            contains: None,
        };
        assert!(!event_matches_filters(&event, &filters));
    }

    #[test]
    fn event_filter_rejects_payload_missing_text() {
        let event = LogEvent {
            line_no: 1,
            raw: String::new(),
            event_type: "worker.started".to_string(),
            run_id: "run-1".to_string(),
            worker_id: "worker-1".to_string(),
            severity_text: "INFO".to_string(),
            severity_number: 9,
            payload_text: "alpha".to_string(),
        };
        let filters = EventFilters {
            run_id: None,
            worker_id: None,
            event_type: None,
            min_severity: None,
            contains: Some("beta".to_string()),
        };
        assert!(!event_matches_filters(&event, &filters));
    }

    #[test]
    fn resolve_log_path_defaults_to_current_dir_when_not_set() {
        let cwd = std::env::current_dir()
            .unwrap_or_else(|error| panic!("failed to read current dir: {error}"));
        let resolved = resolve_log_path(None)
            .unwrap_or_else(|error| panic!("resolve_log_path default failed: {error}"));
        assert_eq!(resolved, default_run_log_path(&cwd));
    }

    #[test]
    fn resolve_log_path_prefers_custom_path() {
        let path = std::path::PathBuf::from("/tmp/custom.log");
        assert_eq!(
            resolve_log_path(Some(path.clone()))
                .unwrap_or_else(|error| panic!("resolve_log_path custom failed: {error}")),
            path
        );
    }

    #[test]
    fn parse_log_event_returns_none_when_type_missing() {
        let value = serde_json::json!({ "payload": { "run_id": "run-1" }});
        assert!(parse_log_event(1, &value, "{}").is_none());
    }

    #[test]
    fn read_log_events_skips_invalid_or_empty_lines() {
        let dir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let path = dir.path().join("otel-logs.jsonl");
        let mut file = std::fs::File::create(&path)
            .unwrap_or_else(|error| panic!("failed to create fixture: {error}"));
        use std::io::Write;
        let result = writeln!(file, "{{}}");
        if let Err(error) = result {
            panic!("failed to write valid json line: {error}");
        }
        let result = writeln!(file, "broken json");
        if let Err(error) = result {
            panic!("failed to write invalid json line: {error}");
        }
        writeln!(
            file,
            "{}",
            serde_json::json!({"event_type":"worker.started","payload":{"run_id":"run-1","worker_id":"w1"}})
        )
        .unwrap_or_else(|error| panic!("failed to write valid event line: {error}"));
        let events = read_log_events(&path)
            .unwrap_or_else(|error| panic!("read_log_events failed: {error}"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "worker.started");
    }

    #[test]
    fn extract_payload_prefers_payload_field() {
        let value = serde_json::json!({"payload":{"a":1},"logRecord":{"attributes":[{"key":"gardener.payload","value":{"stringValue":"{\"a\":2}"}}]}});
        let payload = extract_payload(&value).unwrap_or_else(|| panic!("expected payload field"));
        assert_eq!(payload["a"], 1);
    }

    #[test]
    fn extract_payload_falls_back_to_gardener_payload() {
        let value = serde_json::json!({"logRecord":{"attributes":[{"key":"gardener.payload","value":{"stringValue":"{\"a\":2}"}}]}});
        let payload =
            extract_payload(&value).unwrap_or_else(|| panic!("expected fallback payload"));
        assert_eq!(payload["a"], 2);
    }

    #[test]
    fn extractors_read_from_payload_and_attributes() {
        let payload = serde_json::json!({
            "payload": {
                "run_id":"payload-run",
                "worker_id":"payload-worker",
                "event_type":"payload-event",
            },
            "logRecord": {
                "attributes": [
                    {"key":"run.id","value":{"stringValue":"attr-run"}},
                    {"key":"payload.worker_id","value":{"stringValue":"attr-worker"}},
                    {"key":"event.type","value":{"stringValue":"attr-event"}},
                ]
            }
        });
        assert_eq!(
            extract_event_type(&payload).unwrap_or_else(|| panic!("expected event type")),
            "attr-event"
        );
        assert_eq!(extract_run_id(&payload), "attr-run");
        assert_eq!(extract_worker_id(&payload), "payload-worker");
        assert_eq!(extract_severity(&payload).0, "INFO");
    }

    #[test]
    fn extractors_fallback_to_payload_fields() {
        let value = serde_json::json!({
            "payload": {
                "run_id":"payload-run",
                "worker_id":"payload-worker",
                "event_type":"payload-event",
            }
        });
        assert_eq!(
            extract_event_type(&value).unwrap_or_else(|| panic!("expected event type")),
            "payload-event"
        );
        assert_eq!(extract_run_id(&value), "payload-run");
        assert_eq!(extract_worker_id(&value), "payload-worker");
    }

    #[test]
    fn extract_attr_value_returns_none_without_match() {
        let attrs = [serde_json::json!({"key":"other","value":{"stringValue":"abc"}})];
        assert_eq!(extract_attr_value(&attrs, "missing"), None);
    }
}
