use crate::logging::structured_fallback_line;
use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LogRecord {
    pub source_file: PathBuf,
    pub line_number: usize,
    pub time_unix_nano: u64,
    pub event_type: String,
    pub run_id: String,
    pub worker_id: String,
    pub payload: Value,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileIndex {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub line_count: usize,
    pub first_time_nano: u64,
    pub last_time_nano: u64,
    pub run_ids: Vec<String>,
    pub worker_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunTracePoint {
    pub time: String,
    pub event_type: String,
    pub worker_id: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct StateTransition {
    pub time: String,
    pub worker_id: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorEvent {
    pub time: String,
    pub worker_id: String,
    pub event_type: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunTrace {
    pub run_id: String,
    pub files_spanned: Vec<String>,
    pub first_event: Option<RunTracePoint>,
    pub last_event: Option<RunTracePoint>,
    pub duration_secs: u64,
    pub state_transitions: Vec<StateTransition>,
    pub errors: Vec<ErrorEvent>,
    pub worker_ids: Vec<String>,
    pub event_count: usize,
}

/// Returns log files in chronological order (oldest -> newest).
pub fn discover_log_files(log_path: &Path) -> Vec<PathBuf> {
    let _ = structured_fallback_line("log_query", "discover_log_files", "started");
    let mut files = Vec::new();

    let Some(file_stem) = log_path.file_stem().and_then(|stem| stem.to_str()) else {
        return files;
    };

    let extension = log_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_string();
    let parent = log_path.parent().unwrap_or_else(|| Path::new("."));

    for slot in (1..=3).rev() {
        let rotated = if extension.is_empty() {
            parent.join(format!("{file_stem}.{slot}"))
        } else {
            parent.join(format!("{file_stem}.{slot}.{extension}"))
        };
        if rotated.exists() {
            files.push(rotated);
        }
    }

    if log_path.exists() {
        files.push(log_path.to_path_buf());
    }

    files
}

/// Parse a single JSONL line into a [`LogRecord`].
pub fn parse_log_line(source: &Path, line_num: usize, line: &str) -> Option<LogRecord> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let value: Value = serde_json::from_str(line).ok()?;
    let event_type = extract_event_type(&value)?;

    Some(LogRecord {
        source_file: source.to_path_buf(),
        line_number: line_num,
        time_unix_nano: extract_time_unix_nano(&value),
        event_type,
        run_id: extract_run_id(&value),
        worker_id: extract_worker_id(&value),
        payload: extract_payload(&value),
        raw_line: line.to_string(),
    })
}

/// Compute a `FileIndex` for a single log file.
pub fn index_file(path: &Path) -> FileIndex {
    let _ = structured_fallback_line("log_query", "index_file", "processing");
    let size_bytes = std::fs::metadata(path).map_or(0, |meta| meta.len());
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => {
            return FileIndex {
                path: path.to_path_buf(),
                size_bytes,
                line_count: 0,
                first_time_nano: 0,
                last_time_nano: 0,
                run_ids: Vec::new(),
                worker_ids: Vec::new(),
            };
        }
    };

    let mut line_count = 0usize;
    let mut first_time_nano: Option<u64> = None;
    let mut last_time_nano: Option<u64> = None;
    let mut run_ids = BTreeSet::new();
    let mut worker_ids = BTreeSet::new();

    for (line_no, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(_) => continue,
        };

        let Some(record) = parse_log_line(path, line_no + 1, &line) else {
            continue;
        };

        line_count = line_count.saturating_add(1);

        if first_time_nano.is_none() {
            if record.time_unix_nano > 0 {
                first_time_nano = Some(record.time_unix_nano);
            }
        } else if record.time_unix_nano > 0 && record.time_unix_nano < first_time_nano.unwrap_or(0)
        {
            first_time_nano = Some(record.time_unix_nano);
        }

        if last_time_nano.is_none() {
            if record.time_unix_nano > 0 {
                last_time_nano = Some(record.time_unix_nano);
            }
        } else if record.time_unix_nano >= last_time_nano.unwrap_or(0) {
            last_time_nano = Some(record.time_unix_nano.max(last_time_nano.unwrap_or(0)));
        }

        if !record.run_id.is_empty() {
            run_ids.insert(record.run_id);
        }
        if !record.worker_id.is_empty() {
            worker_ids.insert(record.worker_id);
        }
    }

    let first_time_nano = first_time_nano.unwrap_or(0);
    let last_time_nano = last_time_nano.unwrap_or(0);

    FileIndex {
        path: path.to_path_buf(),
        size_bytes,
        line_count,
        first_time_nano,
        last_time_nano,
        run_ids: run_ids.into_iter().collect(),
        worker_ids: worker_ids.into_iter().collect(),
    }
}

pub fn parse_time_filter(input: &str) -> Option<u64> {
    let raw = input.trim();
    raw.parse::<u64>().ok().or_else(|| {
        DateTime::parse_from_rfc3339(raw).ok().and_then(|dt| {
            let dt = dt.with_timezone(&Utc);
            let secs = dt.timestamp();
            let nanos = u64::from(dt.timestamp_subsec_nanos());
            u64::try_from(secs).ok().and_then(|secs| {
                secs.checked_mul(1_000_000_000)
                    .and_then(|base| base.checked_add(nanos))
            })
        })
    })
}

pub fn format_time(time_unix_nano: u64) -> String {
    let _ = structured_fallback_line("log_query", "format_time", "formatting");
    if time_unix_nano == 0 {
        return "-".to_string();
    }

    let time_nanos = match i64::try_from(time_unix_nano) {
        Ok(value) => value,
        Err(_) => return "-".to_string(),
    };

    let seconds = time_nanos.div_euclid(1_000_000_000);
    let nanos = match u32::try_from(time_nanos.rem_euclid(1_000_000_000)) {
        Ok(value) => value,
        Err(_) => return "-".to_string(),
    };

    Utc.timestamp_opt(seconds, nanos)
        .single()
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| "-".to_string())
}

#[derive(Debug)]
pub struct FilterOptions {
    pub run_id: Option<String>,
    pub worker_id: Option<String>,
    pub event_type_prefix: Option<String>,
    pub since: Option<u64>,
    pub until: Option<u64>,
}

pub fn filter_records(
    log_path: &Path,
    options: FilterOptions,
    file_limit: Option<usize>,
) -> io::Result<Vec<LogRecord>> {
    let _ = structured_fallback_line("log_query", "filter_records", "started");
    let mut files = discover_log_files(log_path);
    if let Some(limit) = file_limit {
        let start = files.len().saturating_sub(limit);
        files = files.into_iter().skip(start).collect();
    }

    let mut matches = Vec::new();
    for path in files {
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(_) => continue,
        };

        for (line_no, line) in BufReader::new(file).lines().enumerate() {
            let line = match line {
                Ok(line) => line,
                Err(_) => continue,
            };
            let Some(record) = parse_log_line(&path, line_no + 1, &line) else {
                continue;
            };
            if record_matches_filter(&record, &options) {
                matches.push(record);
            }
        }
    }

    Ok(matches)
}

fn record_matches_filter(record: &LogRecord, options: &FilterOptions) -> bool {
    let _ = structured_fallback_line("log_query", "record_matches_filter", "evaluating");
    if let Some(run_id) = options.run_id.as_deref() {
        if record.run_id != run_id {
            return false;
        }
    }
    if let Some(worker_id) = options.worker_id.as_deref() {
        if record.worker_id != worker_id {
            return false;
        }
    }
    if let Some(prefix) = options.event_type_prefix.as_deref() {
        if !record.event_type.starts_with(prefix) {
            return false;
        }
    }
    if let Some(since) = options.since {
        if record.time_unix_nano < since {
            return false;
        }
    }
    if let Some(until) = options.until {
        if record.time_unix_nano > until {
            return false;
        }
    }
    true
}

pub fn run_trace(log_path: &Path, run_id: &str) -> io::Result<Option<RunTrace>> {
    let _ = structured_fallback_line("log_query", "run_trace", "started");
    let mut records = Vec::new();
    for path in discover_log_files(log_path) {
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(_) => continue,
        };

        for (line_no, line) in BufReader::new(file).lines().enumerate() {
            let line = match line {
                Ok(line) => line,
                Err(_) => continue,
            };
            let Some(record) = parse_log_line(&path, line_no + 1, &line) else {
                continue;
            };
            if record.run_id == run_id {
                records.push(record);
            }
        }
    }

    if records.is_empty() {
        return Ok(None);
    }

    records.sort_by_key(|record| (record.time_unix_nano, record.line_number));

    let mut files_spanned = Vec::new();
    let mut worker_ids = BTreeSet::new();
    let mut state_transitions = Vec::new();
    let mut errors = Vec::new();

    let mut first_event = None;
    let mut last_event = None;
    let event_count = records.len();
    let mut first_time = 0u64;
    let mut last_time = 0u64;

    for record in records {
        if first_time == 0 {
            first_time = record.time_unix_nano;
        }
        last_time = record.time_unix_nano;

        if let Some(file_name) = record
            .source_file
            .file_name()
            .and_then(|name| name.to_str())
        {
            if !files_spanned.iter().any(|entry| entry == file_name) {
                files_spanned.push(file_name.to_string());
            }
        }

        if !record.worker_id.is_empty() {
            worker_ids.insert(record.worker_id.clone());
        }

        let worker_id = record.worker_id.clone();
        let event_type = record.event_type.clone();
        let payload = record.payload.clone();
        let time = record.time_unix_nano;
        let point = RunTracePoint {
            time: format_time(time),
            event_type: event_type.clone(),
            worker_id: worker_id.clone(),
            payload: payload.clone(),
        };

        if first_event.is_none() {
            first_event = Some(point.clone());
        }
        last_event = Some(point);

        if event_type == "worker.activity.state_changed" {
            let state = payload
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
                .to_string();

            state_transitions.push(StateTransition {
                time: format_time(time),
                worker_id: worker_id.clone(),
                state,
            });
        }

        if errors.len() < 20 && record_is_error(&record) {
            errors.push(ErrorEvent {
                time: format_time(time),
                worker_id: worker_id.clone(),
                event_type: event_type.clone(),
                summary: build_error_summary(&payload),
            });
        }
    }

    let duration_secs = if first_time == 0 {
        0
    } else {
        last_time.saturating_sub(first_time) / 1_000_000_000
    };

    Ok(Some(RunTrace {
        run_id: run_id.to_string(),
        files_spanned,
        first_event,
        last_event,
        duration_secs,
        state_transitions,
        errors,
        worker_ids: worker_ids.into_iter().collect(),
        event_count,
    }))
}

fn build_error_summary(payload: &Value) -> String {
    if let Some(summary) = payload.get("summary").and_then(Value::as_str) {
        return summary.to_string();
    }
    if let Some(details) = payload.get("details").and_then(Value::as_str) {
        return details.to_string();
    }
    if let Some(error) = payload.get("error").and_then(Value::as_str) {
        return error.to_string();
    }

    payload.to_string()
}

fn record_is_error(record: &LogRecord) -> bool {
    let _ = structured_fallback_line("log_query", "record_is_error", "evaluating");
    let severity = serde_json::from_str::<Value>(&record.raw_line)
        .ok()
        .and_then(|value| {
            value
                .pointer("/logRecord/severityNumber")
                .and_then(Value::as_u64)
                .or_else(|| {
                    value
                        .pointer("/logRecord/severityNumber")
                        .and_then(Value::as_str)
                        .and_then(|value| value.parse::<u64>().ok())
                })
        });

    if severity.is_some_and(|severity| severity >= 17) {
        return true;
    }

    let event_type = record.event_type.to_lowercase();
    event_type.contains("error") || event_type.contains("failed")
}

fn extract_payload(value: &Value) -> Value {
    if let Some(payload) = value.get("payload") {
        return payload.clone();
    }

    if let Some(attrs) = value
        .pointer("/logRecord/attributes")
        .and_then(Value::as_array)
    {
        if let Some(raw) = extract_attr_string(attrs, "gardener.payload") {
            if let Ok(payload) = serde_json::from_str::<Value>(&raw) {
                return payload;
            }
        }
    }

    Value::Null
}

fn extract_time_unix_nano(value: &Value) -> u64 {
    value
        .pointer("/logRecord/timeUnixNano")
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .pointer("/logRecord/timeUnixNano")
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(0)
}

fn extract_event_type(value: &Value) -> Option<String> {
    if let Some(event_type) = value.get("event_type").and_then(Value::as_str) {
        return Some(event_type.to_string());
    }

    value
        .pointer("/logRecord/attributes")
        .and_then(Value::as_array)
        .and_then(|attrs| extract_attr_string(attrs, "event.type"))
}

fn extract_run_id(value: &Value) -> String {
    if let Some(attrs) = value
        .pointer("/logRecord/attributes")
        .and_then(Value::as_array)
    {
        if let Some(run_id) = extract_attr_string(attrs, "run.id") {
            return run_id;
        }
    }

    value
        .get("payload")
        .and_then(|payload| payload.get("run_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn extract_worker_id(value: &Value) -> String {
    if let Some(worker_id) = value
        .get("payload")
        .and_then(|payload| payload.get("worker_id").and_then(Value::as_str))
    {
        return worker_id.to_string();
    }

    if let Some(attrs) = value
        .pointer("/logRecord/attributes")
        .and_then(Value::as_array)
    {
        if let Some(worker_id) = extract_attr_string(attrs, "payload.worker_id") {
            return worker_id;
        }
        if let Some(raw) = extract_attr_string(attrs, "gardener.payload") {
            if let Ok(payload) = serde_json::from_str::<Value>(&raw) {
                if let Some(worker_id) = payload.get("worker_id").and_then(Value::as_str) {
                    return worker_id.to_string();
                }
            }
        }
    }

    String::new()
}

fn extract_attr_string(attrs: &[Value], key: &str) -> Option<String> {
    for attr in attrs {
        if attr.get("key").and_then(Value::as_str) != Some(key) {
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
    use std::io::Write;
    use tempfile::tempdir;

    fn write_lines(path: &std::path::Path, lines: &[String]) {
        let mut file = std::fs::File::create(path).expect("create");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
    }

    fn otel_line(run_id: &str, worker_id: &str, event_type: &str, time_unix_nano: u64) -> String {
        serde_json::json!({
            "logRecord": {
                "timeUnixNano": time_unix_nano,
                "severityNumber": 9,
                "severityText": "INFO",
                "attributes": [
                    {"key":"run.id","value":{"stringValue":run_id}},
                    {"key":"payload.worker_id","value":{"stringValue":worker_id}},
                    {"key":"event.type","value":{"stringValue":event_type}},
                    {"key":"gardener.payload","value":{"stringValue":"{\"run_id\":\"".to_owned() + run_id + "\",\"worker_id\":\"" + worker_id + "\",\"state\":\"running\"}"}},
                ]
            },
            "event_type": event_type,
            "payload": {
                "run_id": run_id,
                "worker_id": worker_id,
                "state": "running"
            }
        })
        .to_string()
    }

    #[test]
    fn discover_log_files_skips_missing_slots() {
        let dir = tempdir().expect("tempdir");
        let log_path = dir.path().join("otel-logs.jsonl");

        write_lines(&dir.path().join("otel-logs.3.jsonl"), &["{}".to_string()]);
        write_lines(&dir.path().join("otel-logs.1.jsonl"), &["{}".to_string()]);
        write_lines(&log_path, &["{}".to_string()]);

        assert_eq!(
            discover_log_files(&log_path),
            vec![
                dir.path().join("otel-logs.3.jsonl"),
                dir.path().join("otel-logs.1.jsonl"),
                dir.path().join("otel-logs.jsonl"),
            ]
        );
    }

    #[test]
    fn parse_log_line_valid() {
        let path = std::path::Path::new("/tmp/otel-logs.jsonl");
        let line = otel_line(
            "run-1",
            "worker-1",
            "worker.started",
            1_700_000_000_000_000_000,
        );
        let record = parse_log_line(path, 7, &line).expect("record");

        assert_eq!(record.line_number, 7);
        assert_eq!(record.run_id, "run-1");
        assert_eq!(record.worker_id, "worker-1");
        assert_eq!(record.event_type, "worker.started");
        assert_eq!(record.time_unix_nano, 1_700_000_000_000_000_000);
    }

    #[test]
    fn parse_log_line_missing_fields() {
        let path = std::path::Path::new("/tmp/otel-logs.jsonl");
        let value = serde_json::json!({
            "logRecord": {"timeUnixNano": 1},
            "event_type": "worker.started"
        });

        let record = parse_log_line(path, 1, &value.to_string()).expect("record");
        assert_eq!(record.run_id, "");
        assert_eq!(record.worker_id, "");
        assert_eq!(record.payload, Value::Null);
    }

    #[test]
    fn parse_log_line_blank() {
        let path = std::path::Path::new("/tmp/otel-logs.jsonl");
        assert!(parse_log_line(path, 1, "").is_none());
        assert!(parse_log_line(path, 1, " ").is_none());
    }

    #[test]
    fn parse_log_line_invalid_json() {
        let path = std::path::Path::new("/tmp/otel-logs.jsonl");
        assert!(parse_log_line(path, 1, "not-json").is_none());
    }

    #[test]
    fn parse_log_line_requires_event_type() {
        let path = std::path::Path::new("/tmp/otel-logs.jsonl");
        let payload = serde_json::json!({"payload":{"run_id":"run-1","worker_id":"w1"}});
        assert!(parse_log_line(path, 1, &payload.to_string()).is_none());
    }

    #[test]
    fn index_file_returns_deduplicated_sorted_ids() {
        let dir = tempdir().expect("tempdir");
        let log_path = dir.path().join("otel-logs.jsonl");
        let lines = vec![
            otel_line("run-2", "w2", "worker.started", 200),
            otel_line("run-1", "w1", "worker.started", 100),
            otel_line("run-2", "w1", "worker.started", 300),
        ];
        write_lines(&log_path, &lines);

        let index = index_file(&log_path);

        assert_eq!(index.path, log_path);
        assert_eq!(index.line_count, 3);
        assert_eq!(index.first_time_nano, 100);
        assert_eq!(index.last_time_nano, 300);
        assert_eq!(
            index.run_ids,
            vec!["run-1".to_string(), "run-2".to_string()]
        );
        assert_eq!(index.worker_ids, vec!["w1".to_string(), "w2".to_string()]);
    }

    #[test]
    fn index_file_empty_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.jsonl");
        write_lines(&path, &[]);

        let index = index_file(&path);
        assert_eq!(index.line_count, 0);
        assert_eq!(index.first_time_nano, 0);
        assert_eq!(index.last_time_nano, 0);
        assert!(index.run_ids.is_empty());
        assert!(index.worker_ids.is_empty());
    }

    #[test]
    fn parse_time_filter_supports_rfc3339_and_nanos() {
        let nanos = "1700000000000000000";
        let parsed = parse_time_filter(nanos).expect("parsed nanos");
        assert_eq!(parsed, 1_700_000_000_000_000_000);

        let dt = Utc.timestamp_nanos(1_700_000_000_000_000_000);
        let parsed_rfc3339 = parse_time_filter(&dt.to_rfc3339()).expect("parsed rfc3339");
        assert_eq!(parsed, parsed_rfc3339);
    }

    #[test]
    fn run_trace_builds_summary_for_run() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("otel-logs.jsonl");
        write_lines(
            &path,
            &[
                serde_json::json!({
                    "logRecord": {"timeUnixNano": 1_000_u64, "severityNumber": 9, "severityText":"INFO","attributes":[
                        {"key":"run.id","value":{"stringValue":"run-1"}},
                        {"key":"payload.worker_id","value":{"stringValue":"w1"}},
                        {"key":"event.type","value":{"stringValue":"worker.activity.state_changed"}},
                    ]},
                    "event_type": "worker.activity.state_changed",
                    "payload": {"run_id":"run-1","worker_id":"w1","state":"starting"}
                })
                .to_string(),
                serde_json::json!({
                    "logRecord": {"timeUnixNano": 2_000_u64, "severityNumber": 18, "severityText":"ERROR","attributes":[
                        {"key":"run.id","value":{"stringValue":"run-1"}},
                        {"key":"payload.worker_id","value":{"stringValue":"w1"}},
                        {"key":"event.type","value":{"stringValue":"worker.failed"}},
                    ]},
                    "event_type": "worker.failed",
                    "payload": {"run_id":"run-1","worker_id":"w1","summary":"boom"}
                })
                .to_string(),
            ],
        );

        let trace = run_trace(&path, "run-1")
            .expect("run trace")
            .expect("trace");
        assert_eq!(trace.run_id, "run-1");
        assert_eq!(trace.event_count, 2);
        assert_eq!(trace.state_transitions.len(), 1);
        assert_eq!(trace.errors.len(), 1);
        assert_eq!(trace.worker_ids, vec!["w1".to_string()]);
    }
}
