use crate::errors::GardenerError;
use crate::log_retention::enforce_total_budget;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_DISK_BUDGET_BYTES: u64 = 50 * 1024 * 1024;
pub const DEFAULT_RUN_LOG_RELATIVE_PATH: &str = ".gardener/otel-logs.jsonl";
const PROMPT_LINE_LIMIT: usize = 220;

#[derive(Debug, Clone)]
pub struct JsonlLogger {
    pub path: PathBuf,
    pub max_payload_bytes: usize,
    pub budget_bytes: u64,
}

#[derive(Debug, Clone)]
struct RunLogContext {
    run_id: String,
    trace_id: String,
    span_id: String,
    working_dir: String,
}

static RUN_LOGGER: OnceLock<Mutex<Option<JsonlLogger>>> = OnceLock::new();
static RUN_CONTEXT: OnceLock<Mutex<Option<RunLogContext>>> = OnceLock::new();
static RUN_LOG_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static RUN_LOG_NONCE: AtomicU64 = AtomicU64::new(1);

fn run_logger_slot() -> &'static Mutex<Option<JsonlLogger>> {
    RUN_LOGGER.get_or_init(|| Mutex::new(None))
}

fn run_context_slot() -> &'static Mutex<Option<RunLogContext>> {
    RUN_CONTEXT.get_or_init(|| Mutex::new(None))
}

fn run_log_write_lock() -> &'static Mutex<()> {
    RUN_LOG_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

impl JsonlLogger {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            max_payload_bytes: 4096,
            budget_bytes: DEFAULT_DISK_BUDGET_BYTES,
        }
    }

    pub fn append_json(&self, payload: &Value) -> Result<(), GardenerError> {
        // structured_fallback_line("logging", "jsonl.append", "runtime");
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| GardenerError::Io(e.to_string()))?;
        }
        let line = serde_json::to_string(payload).map_err(|e| GardenerError::Io(e.to_string()))?;
        let mut line = line;
        line.push('\n');

        let _guard = run_log_write_lock().lock().expect("run log write lock");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| GardenerError::Io(e.to_string()))?;
        file.write_all(line.as_bytes())
            .map_err(|e| GardenerError::Io(e.to_string()))?;

        if let Some(parent) = self.path.parent() {
            let _ = enforce_total_budget(parent, self.budget_bytes)?;
        }

        Ok(())
    }
}

pub fn default_run_log_path(working_dir: &Path) -> PathBuf {
    if let Ok(path) = env::var("GARDENER_LOG_PATH") {
        return PathBuf::from(path);
    }

    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        return PathBuf::from(home).join(DEFAULT_RUN_LOG_RELATIVE_PATH);
    }

    working_dir.join(DEFAULT_RUN_LOG_RELATIVE_PATH)
}

pub fn init_run_logger(path: impl AsRef<Path>, working_dir: &Path) -> String {
    let run_id = random_hex(16);
    let context = RunLogContext {
        run_id: run_id.clone(),
        trace_id: random_hex(16),
        span_id: random_hex(8),
        working_dir: working_dir.display().to_string(),
    };

    let mut logger_slot = run_logger_slot().lock().expect("run logger lock");
    *logger_slot = Some(JsonlLogger::new(path));

    let mut context_slot = run_context_slot().lock().expect("run context lock");
    *context_slot = Some(context);
    run_id
}

pub fn clear_run_logger() {
    let mut logger_slot = run_logger_slot().lock().expect("run logger lock");
    *logger_slot = None;
    let mut context_slot = run_context_slot().lock().expect("run context lock");
    *context_slot = None;
}

pub fn set_run_working_dir(path: &Path) {
    let mut context_slot = run_context_slot().lock().expect("run context lock");
    if let Some(context) = context_slot.as_mut() {
        context.working_dir = path.display().to_string();
    }
}

pub fn append_run_log(level: &str, event_type: &str, payload: Value) {
    let logger = {
        let logger_slot = run_logger_slot().lock().expect("run logger lock");
        logger_slot.clone()
    };
    let context = {
        let context_slot = run_context_slot().lock().expect("run context lock");
        context_slot.clone()
    };

    let (Some(logger), Some(context)) = (logger, context) else {
        return;
    };

    let ts_ns = now_unix_nanos();
    let (severity_text, severity_number) = to_otel_severity(level);
    let truncated_payload = truncate_json(payload, logger.max_payload_bytes);
    let payload_string =
        serde_json::to_string(&truncated_payload).unwrap_or_else(|_| "\"<encode-error>\"".into());

    let line = json!({
        "resource": {
            "attributes": [
                kv_attr("service.name", "gardener"),
                kv_attr("service.version", env!("CARGO_PKG_VERSION")),
                kv_attr("service.namespace", "gardener"),
                kv_attr("process.runtime.name", "rust")
            ]
        },
        "scope": {
            "name": "gardener.runtime",
            "version": env!("CARGO_PKG_VERSION")
        },
        "logRecord": {
            "timeUnixNano": ts_ns,
            "observedTimeUnixNano": ts_ns,
            "severityNumber": severity_number,
            "severityText": severity_text,
            "body": {
                "stringValue": event_type
            },
            "attributes": [
                kv_attr("event.type", event_type),
                kv_attr("run.id", &context.run_id),
                kv_attr("run.working_dir", &context.working_dir),
                kv_attr("gardener.payload", &payload_string)
            ],
            "traceId": context.trace_id,
            "spanId": context.span_id,
            "flags": 1
        },
        "event_type": event_type,
        "payload": truncated_payload
    });

    let _ = logger.append_json(&line);
}

pub fn current_run_log_path() -> Option<PathBuf> {
    let logger_slot = run_logger_slot().lock().expect("run logger lock");
    logger_slot.as_ref().map(|logger| logger.path.clone())
}

pub fn current_run_id() -> Option<String> {
    let context_slot = run_context_slot().lock().expect("run context lock");
    context_slot.as_ref().map(|context| context.run_id.clone())
}

pub fn recent_worker_log_lines(worker_id: &str, max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }
    // structured_fallback_line("logging", "recent_worker_log_lines", "starting_filter");

    let Some(path) = current_run_log_path() else {
        return Vec::new();
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };

    let mut lines = text
        .lines()
        .filter_map(|line| {
            let value = serde_json::from_str::<Value>(line).ok()?;
            if !worker_log_is_for_worker(&value, worker_id) {
                return None;
            }
            let event_type = value
                .get("event_type")
                .and_then(Value::as_str)
                .unwrap_or("event");
            let payload = value
                .get("payload")
                .map(serde_json::to_string)
                .unwrap_or_else(|| Ok("{}".to_string()))
                .unwrap_or_else(|_| "<invalid payload>".to_string())
                .replace('\n', "\\n");
            Some(format!(
                "{event_type}: {}",
                truncate_utf8(&payload, PROMPT_LINE_LIMIT)
            ))
        })
        .collect::<Vec<_>>();

    if lines.len() <= max_lines {
        return lines;
    }
    lines.split_off(lines.len() - max_lines).into_iter().collect()
}

pub fn current_log_line_count() -> usize {
    let _ = structured_fallback_line("logging", "current_log_line_count", "starting");
    let Some(path) = current_run_log_path() else {
        return 0;
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return 0,
    };
    text.lines().count()
}

pub fn recent_worker_tool_commands(
    from_line: usize,
    max_lines: usize,
) -> Vec<(usize, String, String)> {
    let _ = structured_fallback_line("logging", "recent_worker_tool_commands", "starting");
    if max_lines == 0 {
        return Vec::new();
    }

    let Some(path) = current_run_log_path() else {
        return Vec::new();
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };

    let mut events = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if idx < from_line {
            continue;
        }
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let event_type = value
            .get("event_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !event_type.starts_with("adapter.") {
            continue;
        }

        let command = match value.get("payload").and_then(extract_payload_command) {
            Some(command) => command,
            None => continue,
        };

        let kind = value
            .get("payload")
            .and_then(|p| p.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if kind != "ToolCall" && kind != "ToolResult" {
            continue;
        }

        let worker_id = match value
            .get("payload")
            .and_then(|p| p.get("worker_id"))
            .and_then(Value::as_str)
        {
            Some(worker_id) => worker_id.to_string(),
            None => continue,
        };
        let command = if command.len() > PROMPT_LINE_LIMIT {
            truncate_utf8(&command, PROMPT_LINE_LIMIT)
        } else {
            command
        };
        events.push((idx, worker_id, command));
    }

    if events.len() <= max_lines {
        return events;
    }
    events.split_off(events.len() - max_lines)
}

fn extract_payload_command(payload: &Value) -> Option<String> {
    payload
        .get("command")
        .and_then(Value::as_str)
        .map(|command| command.replace('\n', "\\n"))
        .or_else(|| {
            payload
                .get("input")
                .and_then(|input| input.get("command"))
                .and_then(Value::as_str)
                .map(|command| command.replace('\n', "\\n"))
        })
        .or_else(|| payload.get("payload").and_then(extract_payload_command))
        .or_else(|| {
            payload
                .get("item")
                .and_then(extract_payload_command)
        })
}

fn kv_attr(key: &str, value: &str) -> Value {
    json!({
        "key": key,
        "value": {
            "stringValue": value
        }
    })
}

fn now_unix_nanos() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .to_string()
}

fn to_otel_severity(level: &str) -> (&'static str, u8) {
    match level.to_ascii_lowercase().as_str() {
        "trace" => ("TRACE", 1),
        "debug" => ("DEBUG", 5),
        "warn" | "warning" => ("WARN", 13),
        "error" => ("ERROR", 17),
        "fatal" => ("FATAL", 21),
        _ => ("INFO", 9),
    }
}

pub fn structured_fallback_line(worker_id: &str, state: &str, message: &str) -> String {
    format!(
        "worker_id={worker_id} state={state} message={} ",
        message.replace('\n', "\\n")
    )
}

fn truncate_json(value: Value, max_bytes: usize) -> Value {
    let rendered = serde_json::to_string(&value).unwrap_or_default();
    if rendered.len() <= max_bytes {
        return value;
    }
    let truncated_payload = truncate_utf8(&rendered, max_bytes);
    Value::String(truncated_payload)
}

fn worker_log_is_for_worker(value: &Value, worker_id: &str) -> bool {
    match value
        .get("payload")
        .and_then(|payload| payload.get("worker_id"))
        .and_then(Value::as_str)
    {
        Some(logged) => logged == worker_id,
        None => false,
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut cutoff = max_bytes.saturating_sub(3);
    while !value.is_char_boundary(cutoff) {
        cutoff = cutoff.saturating_sub(1);
    }
    format!("{}...", &value[..cutoff])
}

fn random_hex(bytes: usize) -> String {
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    let nonce = RUN_LOG_NONCE.fetch_add(1, Ordering::Relaxed);
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    hasher.update(nonce.to_le_bytes());
    hasher.update(now_ns.to_le_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(bytes * 2);
    for byte in digest.iter().take(bytes) {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        append_run_log, clear_run_logger, current_run_id, current_run_log_path, default_run_log_path,
        init_run_logger, structured_fallback_line, JsonlLogger,
    };
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn logger_writes_jsonl() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.jsonl");
        let logger = JsonlLogger::new(&path);
        logger
            .append_json(&json!({"event_type":"tool","payload":{"text":"ok"}}))
            .expect("append");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("\"event_type\":\"tool\""));
    }

    #[test]
    fn otel_log_line_contains_log_record_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("otel.jsonl");
        let run_id = init_run_logger(&path, dir.path());
        append_run_log("info", "run.started", json!({"foo":"bar"}));
        clear_run_logger();
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("\"logRecord\""));
        assert!(text.contains("\"severityText\":\"INFO\""));
        assert!(text.contains(&run_id));
    }

    #[test]
    fn payload_is_truncated_when_needed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("otel.jsonl");
        let _run_id = init_run_logger(&path, dir.path());
        {
            let mut slot = super::run_logger_slot().lock().expect("run logger lock");
            let logger = slot.as_mut().expect("logger initialized");
            logger.max_payload_bytes = 20;
        }
        append_run_log(
            "info",
            "payload.large",
            json!({"text": "abcdefghijklmnopqrstuvwxyz"}),
        );
        clear_run_logger();
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("..."));
    }

    #[test]
    fn current_run_context_functions_report_initialized_run_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("context.jsonl");
        let run_id = init_run_logger(&path, dir.path());
        assert_eq!(current_run_id().as_deref(), Some(run_id.as_str()));
        assert_eq!(current_run_log_path(), Some(path));
        clear_run_logger();
    }

    #[test]
    fn default_path_points_at_cache_file() {
        let path = default_run_log_path(Path::new("/repo"));
        assert!(path.ends_with(".gardener/otel-logs.jsonl"));
    }

    #[test]
    fn logger_enforces_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.jsonl");
        let mut logger = JsonlLogger::new(&path);
        logger.budget_bytes = 1024;
        logger
            .append_json(&json!({"event_type":"tool"}))
            .expect("append");
        assert!(path.exists());
    }

    #[test]
    fn fallback_line_is_deterministic() {
        let line = structured_fallback_line("w1", "doing", "hello\nworld");
        assert_eq!(line, "worker_id=w1 state=doing message=hello\\nworld ");
    }
}
