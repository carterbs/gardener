use gardener::clippy_lints::ClippyLintConfig;
use std::path::Path;

#[test]
fn clippy_expect_used_is_enabled_in_lint_config_and_manifest() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_dir = match manifest_dir.parent().and_then(Path::parent) {
        Some(dir) => dir,
        None => panic!("workspace root should be two levels above package manifest dir"),
    };
    let manifest = match std::fs::read_to_string(manifest_dir.join("Cargo.toml")) {
        Ok(contents) => contents,
        Err(err) => panic!("workspace package manifest must be readable: {err}"),
    };

    assert!(
        manifest.contains("[lints.clippy]") && manifest.contains("expect_used = \"warn\""),
        "expected package lints to configure clippy::expect_used as warn"
    );

    let lint_path = workspace_dir.join("clippy-lints.toml");
    let config = ClippyLintConfig::load(&lint_path)
        .unwrap_or_else(|e| panic!("failed to load clippy-lints.toml: {e}"));

    let has_expect_warn = config.lints.iter().any(|r| {
        r.name == "clippy::expect_used" && r.level == gardener::clippy_lints::LintLevel::Warn
    });

    assert!(
        has_expect_warn,
        "clippy-lints.toml should include clippy::expect_used as warn"
    );
}
