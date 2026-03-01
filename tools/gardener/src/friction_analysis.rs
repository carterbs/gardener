use crate::agent::factory::AdapterFactory;
use crate::agent::AdapterContext;
use crate::backlog_store::NewTask;
use crate::config::AppConfig;
use crate::logging::append_run_log;
use crate::priority::Priority;
use crate::runtime::ProcessRunner;
use crate::task_identity::TaskKind;
use crate::types::RuntimeScope;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
    Completed { findings: Vec<FrictionFinding> },
    Skipped { reason: String },
}

// ---------------------------------------------------------------------------
// 1. Log extraction
// ---------------------------------------------------------------------------

const MAX_TIMELINE_BYTES: usize = 32 * 1024;
const INTERESTING_PREFIXES: &[&str] = &[
    "worker.",
    "agent.turn.",
    "merge_loop.",
    "worker.gitting.",
];
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
        let interesting_type = INTERESTING_PREFIXES.iter().any(|p| event_type.starts_with(p));
        let high_severity = severity_num >= MIN_SEVERITY;

        if !interesting_type && !high_severity {
            continue;
        }

        let compact = compact_payload(&parsed);
        lines.push(format!(
            "{} {}: {}",
            severity_text, event_type, compact
        ));

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

    let prompt = format!(
        "{}Task: {} (id={})\nMerge SHA: {}\n\n{}",
        ANALYSIS_PROMPT,
        input.task_summary,
        input.task_id,
        input.merge_sha.unwrap_or("unknown"),
        timeline
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

    let ctx = AdapterContext {
        worker_id: "friction-analyzer".to_string(),
        session_id: format!("friction-{}", input.worker_id),
        sandbox_id: String::new(),
        model: cfg.seeding.model.clone(),
        cwd,
        prompt_version: "friction-v1".to_string(),
        context_manifest_hash: String::new(),
        output_schema: None,
        output_file: None,
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

    let response: FrictionAnalysisResponse = match serde_json::from_value(result.payload.clone()) {
        Ok(r) => r,
        Err(e) => {
            append_run_log(
                "warn",
                "friction_analysis.parse_failed",
                json!({
                    "worker_id": input.worker_id,
                    "error": e.to_string(),
                    "raw_payload": result.payload.to_string().chars().take(500).collect::<String>()
                }),
            );
            FrictionAnalysisResponse {
                findings: vec![],
                smooth_run: false,
            }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(result.contains("worker.started"), "should include worker.started");
        assert!(result.contains("agent.turn.completed"), "should include agent.turn.completed");
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
        assert!(!result.contains("prompt.rendered"), "should drop prompt.rendered");
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
        let result = extract_worker_timeline(
            Path::new("/nonexistent/otel-logs.jsonl"),
            "run-1",
            "w1",
        )
        .expect("extract timeline from missing file");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_truncates_oversized_results() {
        let dir = tempfile::tempdir().expect("create tempdir");
        // Create many events that exceed 32KB
        let lines: Vec<String> = (0..2000)
            .map(|i| {
                otel_line(
                    "run-1",
                    "w1",
                    &format!("worker.event.{}", i),
                    "INFO",
                    5,
                )
            })
            .collect();
        let path = write_log_file(dir.path(), &lines);

        let result = extract_worker_timeline(&path, "run-1", "w1").expect("extract timeline");
        assert!(
            result.contains("[..."),
            "should contain omission marker"
        );
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
        let resp: FrictionAnalysisResponse = serde_json::from_str(json_str).expect("deserialize response");
        assert_eq!(resp.findings.len(), 1);
        assert!(!resp.smooth_run);
        assert_eq!(resp.findings[0].category, "tool_failure");
    }

    #[test]
    fn response_deserializes_smooth_run() {
        let json_str = r#"{"findings": [], "smooth_run": true}"#;
        let resp: FrictionAnalysisResponse = serde_json::from_str(json_str).expect("deserialize response");
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
}
