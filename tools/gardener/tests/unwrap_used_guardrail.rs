use gardener::clippy_lints::ClippyLintConfig;
use std::path::PathBuf;

#[test]
fn clippy_lints_toml_enforces_unwrap_used_as_deny_for_lib_bins() {
    let lint_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("clippy-lints.toml");

    let config = ClippyLintConfig::load(&lint_path)
        .unwrap_or_else(|e| panic!("failed to load clippy-lints.toml: {e}"));

    let has_deny_unwrap = config.lints.iter().any(|r| {
        r.name == "clippy::unwrap_used"
            && r.level == gardener::clippy_lints::LintLevel::Deny
            && r.scope == gardener::clippy_lints::LintScope::LibBins
    });

    assert!(
        has_deny_unwrap,
        "clippy-lints.toml should enforce unwrap_used with deny level for lib-bins scope",
    );
}
