use std::path::PathBuf;
use std::process::{Command, Stdio};

fn watch_otel_logs_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
        .join("watch-otel-logs.sh")
}

#[test]
fn watch_otel_logs_smoke_defaults_invalid_env_values() {
    let output = {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let log_path = temp_dir.path().join("otel-logs.jsonl");
        std::fs::write(&log_path, "invalid-json-example\n").expect("write otel log fixture");

        let mut cmd = Command::new("timeout");
        cmd.arg("1s")
            .arg("bash")
            .arg(watch_otel_logs_script())
            .env("GARDENER_LOG_PATH", &log_path)
            .env("GARDENER_OTEL_LOG_INTERVAL_SECONDS", "not-a-number")
            .env("GARDENER_OTEL_LOG_TAIL_LINES", "also-not-a-number")
            .env("GARDENER_OTEL_PRETTY", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        cmd.output().expect("run watch-otel-logs smoke command")
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(124),
        "watch-otel-logs should be stopped by timeout"
    );

    assert!(
        stderr.contains(
            "warn: invalid GARDENER_OTEL_LOG_INTERVAL_SECONDS=not-a-number, defaulting to 60"
        ),
        "interval env validation warning should be printed"
    );
    assert!(
        stderr.contains(
            "warn: invalid GARDENER_OTEL_LOG_TAIL_LINES=also-not-a-number, defaulting to 30"
        ),
        "tail lines env validation warning should be printed"
    );
    assert!(
        stdout.contains("Last 30 lines (refresh 60 s)"),
        "fallback tail lines and interval should be reflected in output"
    );
}
