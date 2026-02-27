use crate::errors::GardenerError;
use crate::log_retention::enforce_total_budget;
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const DEFAULT_DISK_BUDGET_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct JsonlLogger {
    pub path: PathBuf,
    pub max_payload_bytes: usize,
    pub budget_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEvent<'a> {
    pub level: &'a str,
    pub event_type: &'a str,
    pub payload: Value,
}

impl JsonlLogger {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            max_payload_bytes: 4096,
            budget_bytes: DEFAULT_DISK_BUDGET_BYTES,
        }
    }

    pub fn append(&self, event: &LogEvent<'_>) -> Result<(), GardenerError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| GardenerError::Io(e.to_string()))?;
        }
        let truncated = truncate_json(event.payload.clone(), self.max_payload_bytes);
        let line = serde_json::to_string(&LogEvent {
            level: event.level,
            event_type: event.event_type,
            payload: truncated,
        })
        .map_err(|e| GardenerError::Io(e.to_string()))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| GardenerError::Io(e.to_string()))?;
        file.write_all(line.as_bytes())
            .map_err(|e| GardenerError::Io(e.to_string()))?;
        file.write_all(b"\n")
            .map_err(|e| GardenerError::Io(e.to_string()))?;

        if let Some(parent) = self.path.parent() {
            let _ = enforce_total_budget(parent, self.budget_bytes)?;
        }

        Ok(())
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
    let mut truncated = rendered;
    truncated.truncate(max_bytes.saturating_sub(3));
    Value::String(format!("{truncated}..."))
}

#[cfg(test)]
mod tests {
    use super::{structured_fallback_line, JsonlLogger, LogEvent};
    use serde_json::json;

    #[test]
    fn logger_truncates_large_payloads_and_writes_jsonl() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.jsonl");
        let mut logger = JsonlLogger::new(&path);
        logger.max_payload_bytes = 20;
        logger.budget_bytes = 1024;

        logger
            .append(&LogEvent {
                level: "info",
                event_type: "tool",
                payload: json!({"text": "abcdefghijklmnopqrstuvwxyz"}),
            })
            .expect("append");

        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("\"event_type\":\"tool\""));
        assert!(text.contains("..."));
    }

    #[test]
    fn fallback_line_is_deterministic() {
        let line = structured_fallback_line("w1", "doing", "hello\nworld");
        assert_eq!(line, "worker_id=w1 state=doing message=hello\\nworld ");
    }
}
