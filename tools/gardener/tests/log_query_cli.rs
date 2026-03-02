use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::io::Write;
use tempfile::tempdir;

fn must_ok<T, E: std::fmt::Display>(value: Result<T, E>, label: &str) -> T {
    match value {
        Ok(value) => value,
        Err(error) => panic!("{label}: {error}"),
    }
}

fn write_lines(path: &std::path::Path, lines: &[String]) {
    let mut file = must_ok(
        std::fs::File::create(path),
        "failed to create otel log fixture",
    );
    for line in lines {
        if let Err(error) = writeln!(file, "{line}") {
            panic!("failed to write line: {error}");
        }
    }
}

fn otel_line(
    run_id: &str,
    worker_id: &str,
    event_type: &str,
    severity: &str,
    severity_num: u8,
) -> String {
    let payload = serde_json::json!({
        "worker_id": worker_id,
        "run_id": run_id,
        "task_id": "task-1"
    });
    let payload_json = must_ok(serde_json::to_string(&payload), "encode payload json");
    let line = serde_json::json!({
        "logRecord": {
            "severityText": severity,
            "severityNumber": severity_num,
            "attributes": [
                { "key": "run.id", "value": { "stringValue": run_id } },
                { "key": "payload.worker_id", "value": { "stringValue": worker_id } },
                { "key": "event.type", "value": { "stringValue": event_type } },
                { "key": "gardener.payload", "value": { "stringValue": payload_json } }
            ]
        },
        "event_type": event_type,
        "payload": payload,
        "payload_json": payload_json
    });
    line.to_string()
}

#[test]
fn events_filter_by_run_worker_and_event_type() {
    let dir = must_ok(tempdir(), "failed to create tempdir");
    let log_path = dir.path().join("otel-logs.jsonl");
    let lines = vec![
        otel_line("run-1", "w1", "worker.started", "INFO", 9),
        otel_line("run-1", "w2", "worker.started", "INFO", 9),
        otel_line("run-1", "w1", "agent.turn.started", "INFO", 9),
        otel_line("run-2", "w1", "worker.started", "INFO", 9),
    ];
    write_lines(&log_path, &lines);
    let log_path = log_path.to_string_lossy().into_owned();

    let output = cargo_bin_cmd!("log-query")
        .args([
            "events",
            "--log-path",
            log_path.as_str(),
            "--run-id",
            "run-1",
            "--worker-id",
            "w1",
            "--event-type",
            "worker.",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout =
        String::from_utf8(output).unwrap_or_else(|_| panic!("stdout should be valid utf8"));
    assert!(
        stdout.contains("worker.started"),
        "expected worker.started event"
    );
    assert!(
        !stdout.contains("agent.turn"),
        "did not expect agent.turn event"
    );
}

#[test]
fn timeline_omits_noise_events() {
    let dir = must_ok(tempdir(), "failed to create tempdir");
    let log_path = dir.path().join("otel-logs.jsonl");
    let lines = vec![
        otel_line("run-1", "w1", "boot.stage.init", "INFO", 5),
        otel_line("run-1", "w1", "worker.started", "INFO", 9),
        otel_line("run-1", "w1", "boring.debug", "DEBUG", 1),
    ];
    write_lines(&log_path, &lines);

    let log_path = log_path.to_string_lossy().into_owned();
    let output = cargo_bin_cmd!("log-query")
        .args([
            "timeline",
            "--log-path",
            &log_path,
            "--run-id",
            "run-1",
            "--worker-id",
            "w1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout =
        String::from_utf8(output).unwrap_or_else(|_| panic!("stdout should be valid utf8"));
    assert!(stdout.contains("worker.started"));
    assert!(!stdout.contains("boot.stage.init"));
    assert!(!stdout.contains("boring.debug"));
}

#[test]
fn stats_aggregates_matching_events() {
    let dir = must_ok(tempdir(), "failed to create tempdir");
    let log_path = dir.path().join("otel-logs.jsonl");
    let lines = vec![
        otel_line("run-1", "w1", "worker.started", "INFO", 9),
        otel_line("run-1", "w1", "worker.started", "INFO", 9),
        otel_line("run-1", "w2", "agent.turn.started", "INFO", 9),
    ];
    write_lines(&log_path, &lines);

    let log_path = log_path.to_string_lossy().into_owned();
    let output = cargo_bin_cmd!("log-query")
        .args(["stats", "--log-path", &log_path, "--run-id", "run-1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout =
        String::from_utf8(output).unwrap_or_else(|_| panic!("stdout should be valid utf8"));
    let parsed: Value = match serde_json::from_str(&stdout) {
        Ok(value) => value,
        Err(error) => panic!("stats json parse failed: {error}"),
    };
    assert_eq!(parsed["total"].as_u64(), Some(3));
    assert_eq!(parsed["workers"]["w1"].as_u64(), Some(2));
    assert_eq!(parsed["workers"]["w2"].as_u64(), Some(1));
    assert_eq!(parsed["event_types"]["worker.started"].as_u64(), Some(2));
}

#[test]
fn events_raw_output_respects_limit() {
    let dir = must_ok(tempdir(), "failed to create tempdir");
    let log_path = dir.path().join("otel-logs.jsonl");
    let lines = vec![
        otel_line("run-1", "w1", "worker.started", "INFO", 9),
        otel_line("run-1", "w1", "agent.turn.started", "INFO", 9),
    ];
    write_lines(&log_path, &lines);

    let log_path = log_path.to_string_lossy().into_owned();
    let output = cargo_bin_cmd!("log-query")
        .args([
            "events",
            "--log-path",
            log_path.as_str(),
            "--raw",
            "--limit",
            "1",
            "--run-id",
            "run-1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout =
        String::from_utf8(output).unwrap_or_else(|_| panic!("stdout should be valid utf8"));
    assert_eq!(stdout.lines().count(), 1);
    assert!(stdout.contains("\"worker_id\":\"w1\""));
}

#[test]
fn events_prints_no_match_message_when_empty() {
    let dir = must_ok(tempdir(), "failed to create tempdir");
    let log_path = dir.path().join("otel-logs.jsonl");
    let lines = vec![otel_line("run-1", "w1", "worker.started", "INFO", 9)];
    write_lines(&log_path, &lines);

    let log_path = log_path.to_string_lossy().into_owned();
    let output = cargo_bin_cmd!("log-query")
        .args([
            "events",
            "--log-path",
            log_path.as_str(),
            "--run-id",
            "missing-run",
        ])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(output).unwrap_or_else(|_| {
        panic!("stderr should be valid utf8");
    });
    assert!(stderr.contains("no events matched filters"));
}

#[test]
fn timeline_limit_takes_most_recent_events() {
    let dir = must_ok(tempdir(), "failed to create tempdir");
    let log_path = dir.path().join("otel-logs.jsonl");
    let lines = vec![
        otel_line("run-1", "w1", "worker.started", "INFO", 9),
        otel_line("run-1", "w2", "worker.started", "INFO", 9),
    ];
    write_lines(&log_path, &lines);

    let log_path = log_path.to_string_lossy().into_owned();
    let output = cargo_bin_cmd!("log-query")
        .args([
            "timeline",
            "--log-path",
            log_path.as_str(),
            "--run-id",
            "run-1",
            "--limit",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout =
        String::from_utf8(output).unwrap_or_else(|_| panic!("stdout should be valid utf8"));
    assert_eq!(stdout.lines().count(), 1);
    assert!(stdout.contains("w2"));
}

#[test]
fn timeline_prints_empty_match_message() {
    let dir = must_ok(tempdir(), "failed to create tempdir");
    let log_path = dir.path().join("otel-logs.jsonl");
    let lines = vec![otel_line("run-1", "w1", "worker.started", "INFO", 9)];
    write_lines(&log_path, &lines);

    let log_path = log_path.to_string_lossy().into_owned();
    let output = cargo_bin_cmd!("log-query")
        .args([
            "timeline",
            "--log-path",
            log_path.as_str(),
            "--run-id",
            "missing-run",
        ])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(output).unwrap_or_else(|_| {
        panic!("stderr should be valid utf8");
    });
    assert!(stderr.contains("no timeline events matched filters"));
}

#[test]
fn stats_shows_unknown_for_missing_ids() {
    let dir = must_ok(tempdir(), "failed to create tempdir");
    let log_path = dir.path().join("otel-logs.jsonl");
    write_lines(
        &log_path,
        &["{\"event_type\":\"worker.started\"}".to_string()],
    );

    let log_path = log_path.to_string_lossy().into_owned();
    let output = cargo_bin_cmd!("log-query")
        .args(["stats", "--log-path", log_path.as_str()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout =
        String::from_utf8(output).unwrap_or_else(|_| panic!("stdout should be valid utf8"));
    let parsed: Value = must_ok(serde_json::from_str(&stdout), "stats json parse failed");
    assert_eq!(parsed["run_ids"]["<unknown>"].as_u64(), Some(1));
    assert_eq!(parsed["workers"]["<unknown>"].as_u64(), Some(1));
    assert_eq!(parsed["event_types"]["worker.started"].as_u64(), Some(1));
}

#[test]
fn command_reports_read_error_for_missing_file() {
    let output = cargo_bin_cmd!("log-query")
        .args([
            "events",
            "--log-path",
            "/tmp/log-query-does-not-exist.jsonl",
            "--run-id",
            "run-1",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(output).unwrap_or_else(|_| {
        panic!("stderr should be valid utf8");
    });
    assert!(stderr.contains("error: failed to read log path"));
}
