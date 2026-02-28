use std::path::Path;

#[test]
fn clippy_expect_used_is_enabled_in_ci_validation_and_manifest() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_dir = match manifest_dir.parent().and_then(Path::parent) {
        Some(dir) => dir,
        None => panic!("workspace root should be two levels above package manifest dir"),
    };
    let manifest = match std::fs::read_to_string(manifest_dir.join("Cargo.toml")) {
        Ok(contents) => contents,
        Err(err) => panic!("workspace package manifest must be readable: {err}"),
    };
    let validate_script = match std::fs::read_to_string(workspace_dir.join("scripts/run-validate.sh")) {
        Ok(contents) => contents,
        Err(err) => panic!("run-validate script must be readable: {err}"),
    };

    assert!(
        manifest.contains("[lints.clippy]") && manifest.contains("expect_used = \"warn\""),
        "expected package lints to configure clippy::expect_used as warn"
    );
    assert!(
        validate_script.contains("-W clippy::expect_used"),
        "expected run-validate command to include clippy::expect_used"
    );
}
