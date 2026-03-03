use crate::agent::factory::AdapterFactory;
use crate::agent::AdapterContext;
use crate::backlog_store::{NewTask, TaskStatus};
use crate::config::AppConfig;
use crate::logging::append_run_log;
use crate::priority::Priority;
use crate::runtime::ProcessRunner;
use crate::task_identity::TaskKind;
use crate::types::RuntimeScope;
use serde::de::Error as DeError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrictionFinding {
    pub category: String,
    pub title: String,
    pub description: String,
    pub severity: String,
    pub evidence_events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrictionAnalysisResponse {
    pub findings: Vec<FrictionFinding>,
    pub smooth_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FrictionOutputEnvelope {
    #[serde(default)]
    schema_version: Option<usize>,
    #[serde(default)]
    state: Option<String>,
    payload: Option<FrictionAnalysisResponse>,
}

pub struct FrictionAnalysisInput<'a> {
    pub worker_id: &'a str,
    pub task_id: &'a str,
    pub task_summary: &'a str,
    pub merge_sha: Option<&'a str>,
    pub run_id: &'a str,
    pub log_path: &'a Path,
}

#[derive(Debug)]
pub enum FrictionAnalysisOutcome {
    Completed {
        findings: Vec<FrictionFinding>,
        smooth_run: bool,
    },
    Skipped {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// 1. Log extraction
// ---------------------------------------------------------------------------

const MAX_TIMELINE_BYTES: usize = 32 * 1024;
const INTERESTING_PREFIXES: &[&str] = &["worker.", "agent.turn.", "merge_loop.", "worker.gitting."];
const NOISE_PREFIXES: &[&str] = &["boot.stage.", "prompt.rendered"];
const MIN_SEVERITY: u8 = 9; // WARN=9, ERROR=13, FATAL=17 in OTEL

/// Extract a human-readable timeline of interesting events for a specific
/// worker run from the OTEL JSONL log file.
pub fn extract_worker_timeline(
    log_path: &Path,
    run_id: &str,
    worker_id: &str,
) -> Result<String, crate::errors::GardenerError> {
    let raw = match std::fs::read_to_string(log_path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(crate::errors::GardenerError::Io(e.to_string())),
    };

    let mut lines = Vec::new();

    for (idx, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                // Malformed line — skip silently
                continue;
            }
        };

        if !matches_run_and_worker(&parsed, run_id, worker_id) {
            continue;
        }

        let severity_num = parsed
            .pointer("/logRecord/severityNumber")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u8;
        let severity_text = parsed
            .pointer("/logRecord/severityText")
            .and_then(Value::as_str)
            .unwrap_or("INFO");
        let event_type = extract_event_type(&parsed).unwrap_or_default();

        // Drop noise
        if NOISE_PREFIXES.iter().any(|p| event_type.starts_with(p)) {
            continue;
        }

        // Keep events that are interesting by type OR by severity
        let interesting_type = INTERESTING_PREFIXES
            .iter()
            .any(|p| event_type.starts_with(p));
        let high_severity = severity_num >= MIN_SEVERITY;

        if !interesting_type && !high_severity {
            continue;
        }

        let compact = compact_payload(&parsed);
        lines.push(format!("{} {}: {}", severity_text, event_type, compact));

        // Track line number for diagnostics
        let _ = idx;
    }

    if lines.is_empty() {
        return Ok(String::new());
    }

    // Truncate if too large: keep first ~40% bytes + last ~40% bytes
    let joined = lines.join("\n");
    if joined.len() <= MAX_TIMELINE_BYTES {
        return Ok(joined);
    }

    let budget = MAX_TIMELINE_BYTES / 2;
    let mut head_end = 0;
    let mut head_bytes = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let next = head_bytes + line.len() + 1; // +1 for newline
        if next > budget {
            break;
        }
        head_bytes = next;
        head_end = i + 1;
    }

    let mut tail_start = lines.len();
    let mut tail_bytes = 0usize;
    for (i, line) in lines.iter().enumerate().rev() {
        let next = tail_bytes + line.len() + 1;
        if next > budget {
            break;
        }
        tail_bytes = next;
        tail_start = i;
    }

    // Prevent overlap
    if tail_start <= head_end {
        tail_start = head_end;
    }

    let omitted = tail_start - head_end;
    let mut truncated = Vec::with_capacity(head_end + 1 + (lines.len() - tail_start));
    truncated.extend_from_slice(&lines[..head_end]);
    if omitted > 0 {
        truncated.push(format!("[... {} events omitted ...]", omitted));
    }
    truncated.extend_from_slice(&lines[tail_start..]);
    Ok(truncated.join("\n"))
}

fn matches_run_and_worker(entry: &Value, run_id: &str, worker_id: &str) -> bool {
    let attrs = match entry.pointer("/logRecord/attributes") {
        Some(Value::Array(arr)) => arr,
        _ => return false,
    };

    let mut run_match = run_id.is_empty();
    let mut worker_match = worker_id.is_empty();

    for attr in attrs {
        let key = attr.get("key").and_then(Value::as_str).unwrap_or_default();
        let val = attr
            .pointer("/value/stringValue")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match key {
            "run.id" if val == run_id => run_match = true,
            // Legacy flat attribute
            "payload.worker_id" if val == worker_id => worker_match = true,
            // Current format: worker_id nested in gardener.payload JSON string
            "gardener.payload" => {
                if let Ok(payload) = serde_json::from_str::<Value>(val) {
                    if payload.get("worker_id").and_then(Value::as_str) == Some(worker_id) {
                        worker_match = true;
                    }
                }
            }
            _ => {}
        }

        if run_match && worker_match {
            return true;
        }
    }

    run_match && worker_match
}

fn extract_event_type(entry: &Value) -> Option<String> {
    let attrs = entry.pointer("/logRecord/attributes")?.as_array()?;
    for attr in attrs {
        if attr.get("key").and_then(Value::as_str) == Some("event.type") {
            return attr
                .pointer("/value/stringValue")
                .and_then(Value::as_str)
                .map(|s| s.to_string());
        }
    }
    None
}

fn compact_payload(entry: &Value) -> String {
    let attrs = match entry.pointer("/logRecord/attributes") {
        Some(Value::Array(arr)) => arr,
        _ => return String::new(),
    };

    let mut parts = Vec::new();
    for attr in attrs {
        let key = attr.get("key").and_then(Value::as_str).unwrap_or_default();
        match key {
            // Current format: all payload fields in a single JSON string
            "gardener.payload" => {
                let val = attr
                    .pointer("/value/stringValue")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Ok(payload) = serde_json::from_str::<Value>(val) {
                    if let Some(obj) = payload.as_object() {
                        for (k, v) in obj {
                            // Skip verbose/meta fields
                            if k == "worker_id" || k == "run_id" {
                                continue;
                            }
                            let s = match v {
                                Value::String(s) => s.clone(),
                                Value::Number(n) => n.to_string(),
                                Value::Bool(b) => b.to_string(),
                                _ => continue,
                            };
                            if !s.is_empty() {
                                parts.push(format!("{k}={s}"));
                            }
                        }
                    }
                }
            }
            // Legacy flat attributes
            k if k.starts_with("payload.") || k == "error" || k == "stderr" => {
                let short_key = k.strip_prefix("payload.").unwrap_or(k);
                let val = attr
                    .pointer("/value/stringValue")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .or_else(|| {
                        attr.pointer("/value/intValue")
                            .and_then(Value::as_i64)
                            .map(|n| n.to_string())
                    })
                    .unwrap_or_default();
                if !val.is_empty() {
                    parts.push(format!("{short_key}={val}"));
                }
            }
            _ => {}
        }
    }
    parts.join(" ")
}

// ---------------------------------------------------------------------------
// 2. Agent analysis
// ---------------------------------------------------------------------------

const ANALYSIS_PROMPT: &str = r#"You are analyzing OTEL log events from a Gardener worker run that successfully completed and merged a PR. Your job is to identify systemic friction — problems that the **repository** should fix so future agent runs are smoother.

## Rules

**REPORT** (friction — things the repo/tooling should fix):
- Missing documentation that caused the agent to guess, retry, or explore unnecessarily
- Repeated tool failures caused by misconfiguration or missing setup
- Coding conventions violated because they were undocumented
- Config issues that caused unnecessary remediation loops
- Flaky infrastructure that required unrelated retries

**IGNORE** (expected costs — normal parts of development):
- Running tests, linters, type-checkers, and validation commands
- Normal CI wait times and merge polling
- Single retries that succeeded quickly
- Review suggestions that improved code quality
- Standard git operations (checkout, commit, push)

**DO NOT DUPLICATE OPEN WORK**
- You will receive an `Already tracked open friction tasks` section.
- If a potential finding is already covered there (even if wording differs), do not emit a new finding.
- Only emit net-new friction that is not already represented by an open friction task.

## Categories
Use exactly one of: `missing_context`, `tool_failure`, `convention_gap`, `documentation_gap`, `config_issue`, `flaky_infra`

## Response format
Respond with valid JSON only, no markdown fencing:
{
  "findings": [
    {
      "category": "one of the categories above",
      "title": "Imperative sentence that becomes a backlog task title",
      "description": "2-3 sentence explanation of the friction and its impact",
      "severity": "high|medium|low",
      "evidence_events": ["relevant log lines from the input"]
    }
  ],
  "smooth_run": false
}

If the run had no systemic friction, return `{"findings": [], "smooth_run": true}`.

## Worker run timeline

"#;

fn open_friction_tasks_prompt_context(cfg: &AppConfig, scope: &RuntimeScope) -> String {
    let db_path = crate::startup::backlog_db_path(cfg, scope);
    let store = match crate::backlog_store::BacklogStore::open(db_path) {
        Ok(store) => store,
        Err(err) => {
            append_run_log(
                "warn",
                "friction_analysis.open_tasks_unavailable",
                json!({ "error": err.to_string() }),
            );
            return "Unavailable (failed to open backlog store).".to_string();
        }
    };

    let tasks = match store.list_tasks() {
        Ok(tasks) => tasks,
        Err(err) => {
            append_run_log(
                "warn",
                "friction_analysis.open_tasks_unavailable",
                json!({ "error": err.to_string() }),
            );
            return "Unavailable (failed to list backlog tasks).".to_string();
        }
    };

    let mut lines: Vec<String> = tasks
        .into_iter()
        .filter(|task| task.source == "friction_analysis")
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Ready
                    | TaskStatus::Leased
                    | TaskStatus::InProgress
                    | TaskStatus::MergePending
                    | TaskStatus::Unresolved
            )
        })
        .map(|task| {
            let details = task
                .details
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let details = if details.chars().count() > 180 {
                let trimmed: String = details.chars().take(180).collect();
                format!("{trimmed}...")
            } else {
                details
            };
            format!(
                "- {} | {} | {} | {}",
                task.task_id, task.scope_key, task.title, details
            )
        })
        .collect();

    lines.sort();
    append_run_log(
        "debug",
        "friction_analysis.open_tasks_loaded",
        json!({ "count": lines.len() }),
    );

    if lines.is_empty() {
        "None".to_string()
    } else {
        lines.join("\n")
    }
}

pub fn run_friction_analysis(
    input: &FrictionAnalysisInput<'_>,
    cfg: &AppConfig,
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
) -> Result<FrictionAnalysisOutcome, crate::errors::GardenerError> {
    if cfg.execution.test_mode {
        return Ok(FrictionAnalysisOutcome::Skipped {
            reason: "test_mode".to_string(),
        });
    }

    let timeline = extract_worker_timeline(input.log_path, input.run_id, input.worker_id)?;
    if timeline.is_empty() {
        append_run_log(
            "debug",
            "friction_analysis.skipped",
            json!({
                "worker_id": input.worker_id,
                "reason": "no matching log events"
            }),
        );
        return Ok(FrictionAnalysisOutcome::Skipped {
            reason: "no matching log events".to_string(),
        });
    }

    let open_friction_tasks = open_friction_tasks_prompt_context(cfg, scope);
    let prompt = format!(
        "{}Task: {} (id={})\nMerge SHA: {}\n\n{}\n\n## Already tracked open friction tasks\n{}\n",
        ANALYSIS_PROMPT,
        input.task_summary,
        input.task_id,
        input.merge_sha.unwrap_or("unknown"),
        timeline,
        open_friction_tasks
    );

    let factory = AdapterFactory::with_defaults();
    let adapter = match factory.get(cfg.seeding.backend) {
        Some(a) => a,
        None => {
            append_run_log(
                "warn",
                "friction_analysis.no_adapter",
                json!({ "backend": format!("{:?}", cfg.seeding.backend) }),
            );
            return Ok(FrictionAnalysisOutcome::Skipped {
                reason: "no adapter for backend".to_string(),
            });
        }
    };

    let cwd = scope
        .repo_root
        .as_ref()
        .unwrap_or(&scope.working_dir)
        .to_path_buf();

    let output_schema = friction_output_schema_path(scope)?;
    let output_file = scope.working_dir.join(format!(
        ".cache/gardener/friction-analysis-output-{}.json",
        input.run_id
    ));
    if let Some(parent) = output_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::errors::GardenerError::Io(format!("create_dir_all {}: {e}", parent.display()))
        })?;
    }

    let ctx = AdapterContext {
        worker_id: "friction-analyzer".to_string(),
        session_id: format!("friction-{}", input.worker_id),
        sandbox_id: String::new(),
        model: cfg.seeding.model.clone(),
        cwd,
        prompt_version: "friction-v1".to_string(),
        context_manifest_hash: String::new(),
        output_schema: Some(output_schema),
        output_file: Some(output_file.clone()),
        permissive_mode: false,
        max_turns: Some(1),
    };

    append_run_log(
        "info",
        "friction_analysis.started",
        json!({
            "worker_id": input.worker_id,
            "task_id": input.task_id,
            "timeline_bytes": timeline.len()
        }),
    );

    let result = adapter.execute(process_runner, &ctx, &prompt, None)?;

    let response = match parse_friction_payload(result.payload) {
        Ok(r) => r,
        Err(event_error) => {
            append_run_log(
                "warn",
                "friction_analysis.parse_from_event_failed",
                json!({
                    "worker_id": input.worker_id,
                    "error": event_error.to_string(),
                }),
            );
            let response =
                parse_friction_payload_from_file(&output_file).map_err(|file_error| {
                    append_run_log(
                        "warn",
                        "friction_analysis.parse_failed",
                        json!({
                            "worker_id": input.worker_id,
                            "event_error": event_error.to_string(),
                            "file_error": file_error.to_string()
                        }),
                    );
                    crate::errors::GardenerError::OutputEnvelope(format!(
                    "friction analysis parse failed for worker {}: event_error={}, file_error={}",
                    input.worker_id,
                    event_error,
                    file_error
                ))
                })?;
            append_run_log(
                "info",
                "friction_analysis.parse_recovered_from_file",
                json!({
                    "worker_id": input.worker_id,
                    "run_id": input.run_id,
                }),
            );
            response
        }
    };

    append_run_log(
        "info",
        "friction_analysis.completed",
        json!({
            "worker_id": input.worker_id,
            "finding_count": response.findings.len(),
            "smooth_run": response.smooth_run
        }),
    );

    Ok(FrictionAnalysisOutcome::Completed {
        findings: response.findings,
        smooth_run: response.smooth_run,
    })
}

// ---------------------------------------------------------------------------
// 3. Backlog creation
// ---------------------------------------------------------------------------

pub fn findings_to_tasks(findings: &[FrictionFinding]) -> Vec<NewTask> {
    findings
        .iter()
        .map(|f| {
            let priority = match f.severity.as_str() {
                "high" | "medium" => Priority::P1,
                _ => Priority::P2,
            };
            NewTask {
                kind: TaskKind::Maintenance,
                title: f.title.clone(),
                details: f.description.clone(),
                rationale: format!(
                    "Friction category: {}. Evidence: {}",
                    f.category,
                    f.evidence_events.join("; ")
                ),
                scope_key: format!("friction:{}", f.category),
                priority,
                source: "friction_analysis".to_string(),
                related_pr: None,
                related_branch: None,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Convenience: default log path
// ---------------------------------------------------------------------------

pub fn default_log_path(scope: &RuntimeScope) -> PathBuf {
    scope.working_dir.join(".gardener/otel-logs.jsonl")
}

fn parse_friction_payload(
    value: serde_json::Value,
) -> Result<FrictionAnalysisResponse, serde_json::Error> {
    if let Ok(payload) = serde_json::from_value::<FrictionAnalysisResponse>(value.clone()) {
        return Ok(payload);
    }
    let envelope: FrictionOutputEnvelope = serde_json::from_value(value)?;
    let payload = envelope.payload.ok_or_else(|| {
        serde_json::Error::custom(format!(
            "friction output envelope payload missing or null: schema_version={:?}, state={:?}",
            envelope.schema_version, envelope.state
        ))
    })?;
    Ok(payload)
}

fn parse_friction_payload_from_file(
    path: &Path,
) -> Result<FrictionAnalysisResponse, crate::errors::GardenerError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) => {
            if err.kind() == ErrorKind::NotFound {
                return Err(crate::errors::GardenerError::OutputEnvelope(format!(
                    "friction output file missing: {}",
                    path.display()
                )));
            }
            return Err(crate::errors::GardenerError::Io(format!(
                "read friction output file {}: {err}",
                path.display()
            )));
        }
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(crate::errors::GardenerError::OutputEnvelope(format!(
            "friction output file was empty: {}",
            path.display()
        )));
    }

    let value = serde_json::from_str::<Value>(trimmed).map_err(|err| {
        crate::errors::GardenerError::OutputEnvelope(format!(
            "friction output file contained invalid JSON at {}: {err}",
            path.display()
        ))
    })?;

    parse_friction_payload(value).map_err(|err| {
        crate::errors::GardenerError::OutputEnvelope(format!(
            "friction output file payload parse failed at {}: {err}",
            path.display()
        ))
    })
}

fn friction_output_schema_path(
    scope: &RuntimeScope,
) -> Result<PathBuf, crate::errors::GardenerError> {
    append_run_log(
        "debug",
        "friction_analysis.schema_path",
        json!({
            "working_dir": scope.working_dir.display().to_string(),
        }),
    );
    let path = scope
        .working_dir
        .join(".cache/gardener/schemas/friction_analysis_output_schema.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::errors::GardenerError::Io(format!("create_dir_all {}: {e}", parent.display()))
        })?;
    }

    let desired = friction_output_schema();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing != desired {
        std::fs::write(&path, desired).map_err(|e| {
            crate::errors::GardenerError::Io(format!("write schema {}: {e}", path.display()))
        })?;
    }
    Ok(path)
}

fn friction_output_schema() -> String {
    r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "schema_version": {
      "type": "integer",
      "const": 1
    },
    "state": {
      "type": "string",
      "const": "friction_analysis"
    },
    "payload": {
      "type": "object",
      "additionalProperties": false,
      "required": ["findings", "smooth_run"],
      "properties": {
        "findings": {
          "type": "array",
          "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["category", "title", "description", "severity", "evidence_events"],
            "properties": {
              "category": {
                "type": "string",
                "enum": ["missing_context", "tool_failure", "convention_gap", "documentation_gap", "config_issue", "flaky_infra"]
              },
              "title": {
                "type": "string",
                "minLength": 1
              },
              "description": {
                "type": "string",
                "minLength": 1
              },
              "severity": {
                "type": "string",
                "enum": ["high", "medium", "low"]
              },
              "evidence_events": {
                "type": "array",
                "items": {
                  "type": "string"
                }
              }
            }
          }
        },
        "smooth_run": {
          "type": "boolean"
        }
      }
    }
  },
  "required": ["schema_version", "state", "payload"]
}"#.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use crate::types::RuntimeScope;
    use std::io::Write;

    fn otel_line(
        run_id: &str,
        worker_id: &str,
        event_type: &str,
        severity: &str,
        severity_num: u8,
    ) -> String {
        serde_json::to_string(&json!({
            "logRecord": {
                "severityText": severity,
                "severityNumber": severity_num,
                "attributes": [
                    { "key": "run.id", "value": { "stringValue": run_id } },
                    { "key": "payload.worker_id", "value": { "stringValue": worker_id } },
                    { "key": "event.type", "value": { "stringValue": event_type } },
                    { "key": "payload.task_id", "value": { "stringValue": "task-1" } }
                ]
            }
        }))
        .expect("serialize otel line")
    }

    fn write_log_file(dir: &Path, lines: &[String]) -> PathBuf {
        let path = dir.join("otel-logs.jsonl");
        let mut file = std::fs::File::create(&path).expect("create log file");
        for line in lines {
            writeln!(file, "{}", line).expect("write log line");
        }
        path
    }

    #[test]
    fn extract_filters_by_worker_and_run() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let lines = vec![
            otel_line("run-1", "w1", "worker.started", "INFO", 5),
            otel_line("run-1", "w2", "worker.started", "INFO", 5),
            otel_line("run-2", "w1", "worker.started", "INFO", 5),
            otel_line("run-1", "w1", "agent.turn.completed", "INFO", 5),
        ];
        let path = write_log_file(dir.path(), &lines);

        let result = extract_worker_timeline(&path, "run-1", "w1").expect("extract timeline");
        assert!(
            result.contains("worker.started"),
            "should include worker.started"
        );
        assert!(
            result.contains("agent.turn.completed"),
            "should include agent.turn.completed"
        );
        assert!(!result.contains("w2"), "should not include w2 events");
        // Should have exactly 2 lines
        assert_eq!(result.lines().count(), 2);
    }

    #[test]
    fn extract_drops_noise_events() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let lines = vec![
            otel_line("run-1", "w1", "boot.stage.init", "INFO", 5),
            otel_line("run-1", "w1", "prompt.rendered.v1", "DEBUG", 1),
            otel_line("run-1", "w1", "worker.task.started", "INFO", 5),
        ];
        let path = write_log_file(dir.path(), &lines);

        let result = extract_worker_timeline(&path, "run-1", "w1").expect("extract timeline");
        assert!(!result.contains("boot.stage"), "should drop boot.stage");
        assert!(
            !result.contains("prompt.rendered"),
            "should drop prompt.rendered"
        );
        assert!(result.contains("worker.task.started"));
    }

    #[test]
    fn extract_keeps_high_severity_regardless_of_type() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let lines = vec![
            otel_line("run-1", "w1", "some.random.event", "WARN", 9),
            otel_line("run-1", "w1", "another.event", "ERROR", 13),
            otel_line("run-1", "w1", "boring.debug", "DEBUG", 1),
        ];
        let path = write_log_file(dir.path(), &lines);

        let result = extract_worker_timeline(&path, "run-1", "w1").expect("extract timeline");
        assert!(result.contains("some.random.event"), "WARN should be kept");
        assert!(result.contains("another.event"), "ERROR should be kept");
        assert!(!result.contains("boring.debug"), "DEBUG should be dropped");
    }

    #[test]
    fn extract_returns_empty_for_missing_file() {
        let result =
            extract_worker_timeline(Path::new("/nonexistent/otel-logs.jsonl"), "run-1", "w1")
                .expect("extract timeline from missing file");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_truncates_oversized_results() {
        let dir = tempfile::tempdir().expect("create tempdir");
        // Create many events that exceed 32KB
        let lines: Vec<String> = (0..2000)
            .map(|i| otel_line("run-1", "w1", &format!("worker.event.{}", i), "INFO", 5))
            .collect();
        let path = write_log_file(dir.path(), &lines);

        let result = extract_worker_timeline(&path, "run-1", "w1").expect("extract timeline");
        assert!(result.contains("[..."), "should contain omission marker");
        assert!(
            result.len() <= MAX_TIMELINE_BYTES + 1024, // some slack for the marker
            "should be within size limit"
        );
    }

    #[test]
    fn extract_handles_malformed_json() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("otel-logs.jsonl");
        let mut file = std::fs::File::create(&path).expect("create log file");
        writeln!(file, "not valid json").expect("write line");
        writeln!(file, "{{}}").expect("write line"); // valid but no matching fields
        writeln!(
            file,
            "{}",
            otel_line("run-1", "w1", "worker.started", "INFO", 5)
        )
        .expect("write line");

        let result = extract_worker_timeline(&path, "run-1", "w1").expect("extract timeline");
        assert!(result.contains("worker.started"));
        assert_eq!(result.lines().count(), 1);
    }

    #[test]
    fn response_deserializes_valid_json() {
        let json_str = r#"{
            "findings": [{
                "category": "tool_failure",
                "title": "Fix broken lint config",
                "description": "Lint failed 3 times due to missing .eslintrc",
                "severity": "high",
                "evidence_events": ["WARN worker.doing: lint_exit_code=1"]
            }],
            "smooth_run": false
        }"#;
        let resp: FrictionAnalysisResponse =
            serde_json::from_str(json_str).expect("deserialize response");
        assert_eq!(resp.findings.len(), 1);
        assert!(!resp.smooth_run);
        assert_eq!(resp.findings[0].category, "tool_failure");
    }

    #[test]
    fn response_deserializes_smooth_run() {
        let json_str = r#"{"findings": [], "smooth_run": true}"#;
        let resp: FrictionAnalysisResponse =
            serde_json::from_str(json_str).expect("deserialize response");
        assert!(resp.findings.is_empty());
        assert!(resp.smooth_run);
    }

    #[test]
    fn response_from_malformed_json_returns_default() {
        let val: Value = json!({"unexpected": "shape"});
        let result: Result<FrictionAnalysisResponse, _> = serde_json::from_value(val);
        assert!(result.is_err());
    }

    #[test]
    fn parse_friction_payload_errors_on_null_envelope_payload() {
        let val = serde_json::json!({
            "schema_version": 1,
            "state": "friction_analysis",
            "payload": null
        });
        assert!(parse_friction_payload(val).is_err());
    }

    #[test]
    fn findings_to_tasks_maps_severity_to_priority() {
        let findings = vec![
            FrictionFinding {
                category: "tool_failure".to_string(),
                title: "Fix lint config".to_string(),
                description: "desc".to_string(),
                severity: "high".to_string(),
                evidence_events: vec!["event1".to_string()],
            },
            FrictionFinding {
                category: "convention_gap".to_string(),
                title: "Document naming convention".to_string(),
                description: "desc".to_string(),
                severity: "medium".to_string(),
                evidence_events: vec![],
            },
            FrictionFinding {
                category: "documentation_gap".to_string(),
                title: "Add API docs".to_string(),
                description: "desc".to_string(),
                severity: "low".to_string(),
                evidence_events: vec![],
            },
        ];

        let tasks = findings_to_tasks(&findings);
        assert_eq!(tasks.len(), 3);

        assert_eq!(tasks[0].priority, Priority::P1); // high -> P1
        assert_eq!(tasks[0].kind, TaskKind::Maintenance);
        assert_eq!(tasks[0].source, "friction_analysis");
        assert_eq!(tasks[0].scope_key, "friction:tool_failure");
        assert!(tasks[0].related_pr.is_none());

        assert_eq!(tasks[1].priority, Priority::P1); // medium -> P1

        assert_eq!(tasks[2].priority, Priority::P2); // low -> P2
        assert_eq!(tasks[2].scope_key, "friction:documentation_gap");
    }

    #[test]
    fn findings_to_tasks_returns_empty_for_empty_findings() {
        let tasks = findings_to_tasks(&[]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn run_friction_analysis_recovers_from_null_payload_with_output_file() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/cwd"),
            repo_root: None,
            working_dir: dir.path().to_path_buf(),
        };
        let output_file = scope
            .working_dir
            .join(".cache/gardener/friction-analysis-output-run-1.json");
        if let Some(parent) = output_file.parent() {
            std::fs::create_dir_all(parent).expect("create output dir");
        }
        std::fs::write(
            &output_file,
            r#"{"schema_version":1,"state":"friction_analysis","payload":{"findings":[{"category":"tool_failure","title":"Persisted output fallback","description":"Null payload from terminal event should be replaced by file output.","severity":"high","evidence_events":["adapter.payload.null"]}],"smooth_run":false}}"#,
        )
        .expect("write friction output file");

        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":null}\n".to_string(),
            stderr: String::new(),
        }));

        let log_path = write_log_file(
            dir.path(),
            &[otel_line("run-1", "worker-1", "worker.started", "INFO", 5)],
        );
        let input = FrictionAnalysisInput {
            worker_id: "worker-1",
            task_id: "task-1",
            task_summary: "Test summary",
            merge_sha: None,
            run_id: "run-1",
            log_path: &log_path,
        };
        let outcome = run_friction_analysis(&input, &AppConfig::default(), &runner, &scope)
            .expect("run friction analysis");
        let (findings, smooth_run) = match outcome {
            FrictionAnalysisOutcome::Completed {
                findings,
                smooth_run,
            } => (findings, smooth_run),
            FrictionAnalysisOutcome::Skipped { reason } => {
                panic!("did not expect skip: {reason}")
            }
        };
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Persisted output fallback");
        assert!(!smooth_run);

        let spawned = runner.spawned();
        assert_eq!(spawned.len(), 1);
        let args = &spawned[0].args;
        let schema_index = args
            .iter()
            .position(|arg| arg == "--output-schema")
            .expect("schema arg missing");
        let file_index = args
            .iter()
            .position(|arg| arg == "-o")
            .expect("output file arg missing");
        assert_eq!(
            args[file_index + 1],
            output_file.to_string_lossy().to_string()
        );
        assert!(args[schema_index + 1].ends_with("friction_analysis_output_schema.json"));
    }

    #[test]
    fn run_friction_analysis_recovers_from_null_envelope_payload_with_output_file() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/cwd"),
            repo_root: None,
            working_dir: dir.path().to_path_buf(),
        };
        let output_file = scope
            .working_dir
            .join(".cache/gardener/friction-analysis-output-run-1.json");
        if let Some(parent) = output_file.parent() {
            std::fs::create_dir_all(parent).expect("create output dir");
        }
        std::fs::write(
            &output_file,
            r#"{"schema_version":1,"state":"friction_analysis","payload":{"findings":[{"category":"documentation_gap","title":"Recover from null envelope","description":"Output event had null payload inside an envelope.","severity":"high","evidence_events":["adapter.payload.null"]}],"smooth_run":false}}"#,
        )
        .expect("write friction output file");

        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":{\"schema_version\":1,\"state\":\"friction_analysis\",\"payload\":null}}\n".to_string(),
            stderr: String::new(),
        }));

        let log_path = write_log_file(
            dir.path(),
            &[otel_line("run-1", "worker-1", "worker.started", "INFO", 5)],
        );
        let input = FrictionAnalysisInput {
            worker_id: "worker-1",
            task_id: "task-1",
            task_summary: "Test summary",
            merge_sha: None,
            run_id: "run-1",
            log_path: &log_path,
        };
        let outcome = run_friction_analysis(&input, &AppConfig::default(), &runner, &scope)
            .expect("run friction analysis");
        let (findings, smooth_run) = match outcome {
            FrictionAnalysisOutcome::Completed {
                findings,
                smooth_run,
            } => (findings, smooth_run),
            FrictionAnalysisOutcome::Skipped { reason } => {
                panic!("did not expect skip: {reason}")
            }
        };
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Recover from null envelope");
        assert!(!smooth_run);
        let spawned = runner.spawned();
        assert_eq!(spawned.len(), 1);
        let args = &spawned[0].args;
        let schema_index = args
            .iter()
            .position(|arg| arg == "--output-schema")
            .expect("schema arg missing");
        assert!(args[schema_index + 1].ends_with("friction_analysis_output_schema.json"));
    }

    #[test]
    fn run_friction_analysis_keeps_smooth_run_false_when_payload_is_null() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/cwd"),
            repo_root: None,
            working_dir: dir.path().to_path_buf(),
        };
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "{\"type\":\"turn.completed\",\"result\":null}\n".to_string(),
            stderr: String::new(),
        }));

        let log_path = write_log_file(
            dir.path(),
            &[otel_line("run-1", "worker-1", "worker.started", "INFO", 5)],
        );
        let input = FrictionAnalysisInput {
            worker_id: "worker-1",
            task_id: "task-1",
            task_summary: "Test summary",
            merge_sha: None,
            run_id: "run-1",
            log_path: &log_path,
        };
        let err = run_friction_analysis(&input, &AppConfig::default(), &runner, &scope)
            .expect_err("run friction analysis should fail without fallback");
        assert!(err
            .to_string()
            .contains("friction analysis parse failed for worker worker-1"));
    }

    #[test]
    fn parse_friction_payload_from_file_reads_and_parses_output() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("friction-analysis-output.json");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"state":"friction_analysis","payload":{"findings":[],"smooth_run":true}}"#,
        )
        .expect("write output file");
        let payload = parse_friction_payload_from_file(&path).expect("payload from file");
        assert!(payload.smooth_run);
        assert!(payload.findings.is_empty());
    }

    #[test]
    fn parse_friction_payload_from_file_errors_on_null_payload() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("friction-analysis-output.json");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"state":"friction_analysis","payload":null}"#,
        )
        .expect("write output file");
        assert!(parse_friction_payload_from_file(&path).is_err());
    }

    #[test]
    fn friction_output_schema_is_strict() {
        let schema: serde_json::Value =
            serde_json::from_str(&super::friction_output_schema()).expect("valid JSON schema");
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
        assert_eq!(
            schema["properties"]["payload"]["additionalProperties"],
            serde_json::json!(false)
        );
        assert_eq!(
            schema["properties"]["payload"]["properties"]["findings"]["items"]
                ["additionalProperties"],
            serde_json::json!(false)
        );
    }
}
