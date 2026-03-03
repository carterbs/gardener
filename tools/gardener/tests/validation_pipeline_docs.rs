use std::fs;
use std::path::{Path, PathBuf};

fn repo_root_path() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("crate should live under <repo_root>/tools/gardener")
        .to_path_buf()
}

#[test]
fn validation_pipeline_docs_match_live_validation_hook() {
    let repo_root = repo_root_path();

    let workflow = fs::read_to_string(repo_root.join("docs/conventions/workflow.md"))
        .unwrap_or_else(|err| panic!("failed to read docs/conventions/workflow.md: {err}"));
    let pre_commit = fs::read_to_string(repo_root.join(".githooks/pre-commit"))
        .unwrap_or_else(|err| panic!("failed to read .githooks/pre-commit: {err}"));

    assert!(
        workflow.contains("scripts/doc-gardening.sh"),
        "workflow doc should include doc-gardening maintenance command"
    );
    assert!(
        workflow.contains("scripts/run-script-lint-fixture-tests.sh"),
        "workflow doc should include fixture-script lint command"
    );
    assert!(
        workflow.contains("Validation and pre-commit flow"),
        "workflow doc should include validation/pre-commit section"
    );
    assert!(
        workflow.contains(".githooks/pre-commit"),
        "workflow doc should reference the repository pre-commit hook path"
    );
    assert!(
        workflow.contains("Re-stage updates (`git add`), then retry commit."),
        "workflow should include remediation re-staging guidance"
    );
    assert!(
        pre_commit.contains("rustfmt --edition 2021"),
        "pre-commit hook should auto-format staged Rust files"
    );
    assert!(
        pre_commit.contains("scripts/run-validate.sh"),
        "pre-commit hook should run the canonical validation pipeline"
    );
}

#[test]
fn coverage_ignore_manifest_is_implemented() {
    let repo_root = repo_root_path();

    let coverage_script = fs::read_to_string(repo_root.join("scripts/test-gardener-coverage.sh"))
        .unwrap_or_else(|err| panic!("failed to read scripts/test-gardener-coverage.sh: {err}"));

    assert!(
        coverage_script.contains("COVERAGE_IGNORE_MANIFEST"),
        "coverage script should implement COVERAGE_IGNORE_MANIFEST support"
    );
    assert!(
        coverage_script.contains("--ignore-filename-regex"),
        "coverage script should pass regex filters through llvm-cov"
    );
}
