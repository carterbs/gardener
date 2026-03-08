use assert_cmd::cargo::cargo_bin_cmd;
fn otel_logs_cmd() -> assert_cmd::Command {
    let mut cmd = cargo_bin_cmd!("gardener");
    cmd.arg("otel-logs");
    cmd
}
use serde_json::Value;
use std::io::Write;
use tempfile::tempdir;

fn write_lines(path: &std::path::Path, lines: &[String]) {
    let mut file = std::fs::File::create(path).expect("create log fixture");
    for line in lines {
        writeln!(file, "{line}").expect("write line");
    }
}

fn otel_line(
    run_id: &str,
    worker_id: &str,
    event_type: &str,
    time_unix_nano: u64,
    state: Option<&str>,
    severity_number: u64,
) -> String {
    let mut payload = serde_json::json!({"run_id": run_id, "worker_id": worker_id});
    if let Some(state) = state {
        payload["state"] = serde_json::Value::String(state.to_string());
    }

    let payload_json = serde_json::to_string(&payload).expect("payload json");

    serde_json::json!({
        "logRecord": {
            "timeUnixNano": time_unix_nano,
            "severityNumber": severity_number,
            "severityText": if severity_number >= 17 { "ERROR" } else { "INFO" },
            "attributes": [
                {"key": "run.id", "value": {"stringValue": run_id}},
                {"key": "payload.worker_id", "value": {"stringValue": worker_id}},
                {"key": "event.type", "value": {"stringValue": event_type}},
                {"key": "gardener.payload", "value": {"stringValue": payload_json}},
            ]
        },
        "event_type": event_type,
        "payload": payload,
    })
    .to_string()
}

#[test]
fn index_lists_metadata_for_rotated_files() {
    let dir = tempdir().expect("tempdir");
    let base = dir.path().join("otel-logs.jsonl");
    let rotated = dir.path().join("otel-logs.1.jsonl");

    write_lines(
        &rotated,
        &[otel_line(
            "run-1",
            "w1",
            "worker.started",
            1_000_000_000,
            None,
            9,
        )],
    );
    write_lines(
        &base,
        &[
            otel_line("run-2", "w2", "worker.started", 2_000_000_000, None, 9),
            otel_line(
                "run-1",
                "w1",
                "worker.error",
                3_000_000_000,
                Some("boom"),
                17,
            ),
        ],
    );

    let output = otel_logs_cmd()
        .args(["index", "--log-path", base.to_str().expect("path")])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("stdout");

    assert!(stdout.contains("otel-logs.1.jsonl"));
    assert!(stdout.contains("otel-logs.jsonl"));
    assert!(stdout.contains("run(s)"));
}

#[test]
fn index_json_mode_outputs_valid_json() {
    let dir = tempdir().expect("tempdir");
    let base = dir.path().join("otel-logs.jsonl");
    write_lines(
        &base,
        &[otel_line("run-1", "w1", "worker.started", 1, None, 9)],
    );

    let output = otel_logs_cmd()
        .args([
            "index",
            "--log-path",
            base.to_str().expect("path"),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("json");
    let array = parsed.as_array().expect("array");
    assert_eq!(array.len(), 1);
}

#[test]
fn index_empty_directory_prints_header_only() {
    let dir = tempdir().expect("tempdir");
    let missing = dir.path().join("no-logs-here.jsonl");

    let output = otel_logs_cmd()
        .args(["index", "--log-path", missing.to_str().expect("path")])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("stdout");
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "FILE\tSIZE\tLINES\tFROM\tTO\tRUNS\tWORKERS");
}

#[test]
fn filter_run_id_spans_rotation_boundary() {
    let dir = tempdir().expect("tempdir");
    let base = dir.path().join("otel-logs.jsonl");
    let rotated = dir.path().join("otel-logs.1.jsonl");

    write_lines(
        &rotated,
        &[otel_line(
            "run-1",
            "w1",
            "worker.started",
            1_000_000_000,
            None,
            9,
        )],
    );
    write_lines(
        &base,
        &[
            otel_line(
                "run-1",
                "w1",
                "worker.activity.state_changed",
                2_000_000_000,
                Some("running"),
                9,
            ),
            otel_line("run-2", "w2", "worker.started", 3_000_000_000, None, 9),
        ],
    );

    let output = otel_logs_cmd()
        .args([
            "filter",
            "--log-path",
            base.to_str().expect("path"),
            "--run-id",
            "run-1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output = String::from_utf8(output).expect("stdout");
    let lines: Vec<_> = output.lines().collect();
    assert_eq!(lines.len(), 2);
}

#[test]
fn filter_max_limits_output() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("otel-logs.jsonl");
    write_lines(
        &path,
        &(1..11)
            .map(|i| otel_line("run-1", "w1", "worker.started", i, None, 9))
            .collect::<Vec<_>>(),
    );

    let output = otel_logs_cmd()
        .args([
            "filter",
            "--log-path",
            path.to_str().expect("path"),
            "--run-id",
            "run-1",
            "--max",
            "5",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("json");
    let arr = parsed.as_array().expect("array");
    assert_eq!(arr.len(), 5);
}

#[test]
fn filter_tail_returns_last_matches_in_chronological_order() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("otel-logs.jsonl");
    write_lines(
        &path,
        &[
            otel_line("run-1", "w1", "worker.started", 1, None, 9),
            otel_line("run-1", "w1", "worker.started", 2, None, 9),
            otel_line("run-1", "w1", "worker.started", 3, None, 9),
            otel_line("run-1", "w1", "worker.started", 4, None, 9),
        ],
    );

    let output = otel_logs_cmd()
        .args([
            "filter",
            "--log-path",
            path.to_str().expect("path"),
            "--run-id",
            "run-1",
            "--tail",
            "--max",
            "2",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output = String::from_utf8(output).expect("stdout");
    let lines: Vec<_> = output.lines().collect();
    assert_eq!(lines.len(), 2);

    let first: Value = serde_json::from_str(lines[0]).expect("event1");
    let second: Value = serde_json::from_str(lines[1]).expect("event2");
    let Some(first_time) = first["time_rfc3339"].as_str() else {
        panic!("time missing for first event");
    };
    let Some(second_time) = second["time_rfc3339"].as_str() else {
        panic!("time missing for second event");
    };
    assert!(first_time < second_time);
}

#[test]
fn run_trace_identifies_files_spanned_and_errors() {
    let dir = tempdir().expect("tempdir");
    let rotated = dir.path().join("otel-logs.1.jsonl");
    let base = dir.path().join("otel-logs.jsonl");

    write_lines(
        &rotated,
        &[otel_line(
            "run-1",
            "w1",
            "worker.activity.state_changed",
            1,
            Some("starting"),
            9,
        )],
    );
    write_lines(
        &base,
        &[
            otel_line("run-1", "w1", "worker.started", 2, None, 9),
            otel_line("run-1", "w1", "worker.failed", 3, Some("boom"), 17),
        ],
    );

    let output = otel_logs_cmd()
        .args([
            "run-trace",
            "--log-path",
            base.to_str().expect("path"),
            "--run-id",
            "run-1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("stdout");
    let value: Value = serde_json::from_str(&stdout).expect("json");

    assert_eq!(value["run_id"], "run-1");
    assert_eq!(value["files_spanned"].as_array().expect("files").len(), 2);
    assert_eq!(
        value["files_spanned"].as_array().expect("files")[0],
        "otel-logs.1.jsonl",
    );
    assert_eq!(
        value["files_spanned"].as_array().expect("files")[1],
        "otel-logs.jsonl",
    );
    assert_eq!(
        value["state_transitions"].as_array().expect("states").len(),
        1
    );
    assert_eq!(value["errors"].as_array().expect("errors").len(), 1);
}

#[test]
fn run_trace_missing_run_exits_1() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("otel-logs.jsonl");
    write_lines(
        &path,
        &[otel_line("run-1", "w1", "worker.started", 1, None, 9)],
    );

    otel_logs_cmd()
        .args([
            "run-trace",
            "--log-path",
            path.to_str().expect("path"),
            "--run-id",
            "missing",
        ])
        .assert()
        .failure();
}
