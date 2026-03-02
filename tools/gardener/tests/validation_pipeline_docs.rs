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

    let readme = fs::read_to_string(repo_root.join("README.md"))
        .unwrap_or_else(|err| panic!("failed to read README.md: {err}"));
    let workflow = fs::read_to_string(repo_root.join("docs/conventions/workflow.md"))
        .unwrap_or_else(|err| panic!("failed to read docs/conventions/workflow.md: {err}"));
    let pre_commit = fs::read_to_string(repo_root.join(".githooks/pre-commit"))
        .unwrap_or_else(|err| panic!("failed to read .githooks/pre-commit: {err}"));

    assert!(
        readme.contains("scripts/check-binary-blobs.sh"),
        "README validation pipeline should list the binary-blob linter"
    );
    assert!(
        readme.contains("scripts/run-script-lint-fixture-tests.sh"),
        "README validation pipeline should list fixture-script lint command"
    );
    assert!(
        workflow.contains("scripts/run-script-lint-fixture-tests.sh"),
        "workflow doc should include fixture-script lint command"
    );
    assert!(
        readme.contains("./.githooks/pre-commit"),
        "README should document how to run pre-commit remediation directly"
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
        pre_commit.contains("rustfmt --edition 2021"),
        "pre-commit hook should auto-format staged Rust files"
    );
    assert!(
        pre_commit.contains("scripts/run-validate.sh"),
        "pre-commit hook should run the canonical validation pipeline"
    );
}
