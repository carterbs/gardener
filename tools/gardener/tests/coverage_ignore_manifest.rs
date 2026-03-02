use std::path::PathBuf;
use std::process::Command;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn coverage_script_path() -> PathBuf {
    let mut script_path = crate_root();
    script_path.pop();
    script_path.pop();
    script_path.push("scripts/test-gardener-coverage.sh");
    script_path
}

fn fixture_manifest_path() -> PathBuf {
    let mut path = crate_root();
    path.push("tests/fixtures/coverage-ignore-manifest.txt");
    path
}

#[test]
fn coverage_ignore_manifest_builds_ignore_regex_in_dry_run_mode() {
    let output = Command::new("bash")
        .arg(coverage_script_path())
        .env("COVERAGE_DRY_RUN", "1")
        .env("COVERAGE_IGNORE_MANIFEST", fixture_manifest_path())
        .env_remove("COVERAGE_IGNORE_REGEX")
        .output()
        .expect("coverage gate script should execute in dry-run mode");

    assert!(
        output.status.success(),
        "dry-run must succeed with reviewed manifest"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected =
        "coverage gate dry-run: cargo llvm-cov -p gardener --all-targets --summary-only --ignore-filename-regex /(tools/gardener/src/bin/do_task\\.rs|tools/gardener/src/worker_pool\\.rs|tools/gardener/src/startup\\.rs)";
    assert!(
        stdout.contains(expected),
        "expected manifest-derived ignore regex in dry-run output, got:\n{stdout}"
    );
}

#[test]
fn coverage_ignore_regex_env_takes_precedence() {
    let output = Command::new("bash")
        .arg(coverage_script_path())
        .env("COVERAGE_DRY_RUN", "1")
        .env("COVERAGE_IGNORE_REGEX", "/override/regex")
        .env("COVERAGE_IGNORE_MANIFEST", fixture_manifest_path())
        .output()
        .expect("coverage gate script should execute with env override");

    assert!(
        output.status.success(),
        "dry-run should succeed with explicit COVERAGE_IGNORE_REGEX"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "coverage gate dry-run: cargo llvm-cov -p gardener --all-targets --summary-only --ignore-filename-regex /override/regex"
        ),
        "expected explicit regex override to be used, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("tools/gardener/src/bin/do_task\\.rs"),
        "manifest patterns should not override explicit regex"
    );
}

#[test]
fn coverage_missing_manifest_errors_with_message() {
    let missing_manifest = {
        let mut path = crate_root();
        path.pop();
        path.pop();
        path.push("scripts/coverage-ignore-manifest-does-not-exist.txt");
        path
    };
    let output = Command::new("bash")
        .arg(coverage_script_path())
        .env("COVERAGE_DRY_RUN", "1")
        .env("COVERAGE_IGNORE_MANIFEST", missing_manifest)
        .env_remove("COVERAGE_IGNORE_REGEX")
        .output()
        .expect("coverage gate script should execute and report missing manifest");

    assert!(
        !output.status.success(),
        "script should fail when manifest is missing"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing coverage ignore manifest"),
        "expected missing-manifest diagnostic, got:\n{stderr}"
    );
}
