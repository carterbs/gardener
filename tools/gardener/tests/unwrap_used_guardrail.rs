use std::fs;
use std::path::PathBuf;

#[test]
fn run_validate_enforces_unwrap_used_as_error_for_lib_and_bins() {
    let script_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts/run-validate.sh");

    let script = match fs::read_to_string(&script_path) {
        Ok(script) => script,
        Err(err) => panic!("failed to read {}: {err}", script_path.display()),
    };

    assert!(
        script.contains("cargo clippy -p gardener --lib --bins -- -D clippy::unwrap_used"),
        "run-validate should enforce unwrap_used with an error-level clippy lint",
    );
}
