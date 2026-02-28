use std::path::{Path, PathBuf};

use toml::Value;

#[test]
fn workspace_clippy_lint_configuration_enforces_identity_op_deny() {
    let mut manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_path.pop();
    manifest_path.pop();
    manifest_path.push("Cargo.toml");

    let manifest_text = std::fs::read_to_string(Path::new(&manifest_path))
        .unwrap_or_else(|_| panic!("failed to read workspace manifest: {}", manifest_path.display()));

    let manifest: Value = toml::from_str(&manifest_text).expect("workspace Cargo.toml should parse as TOML");

    let level = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(|lints| lints.get("clippy"))
        .and_then(|clippy| clippy.get("identity_op"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.identity_op is not configured");

    assert_eq!(level, "deny");
}

#[test]
fn workspace_clippy_lint_configuration_enforces_manual_clamp_warn() {
    let mut manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_path.pop();
    manifest_path.pop();
    manifest_path.push("Cargo.toml");

    let manifest_text = std::fs::read_to_string(Path::new(&manifest_path))
        .unwrap_or_else(|_| panic!("failed to read workspace manifest: {}", manifest_path.display()));

    let manifest: Value = toml::from_str(&manifest_text).expect("workspace Cargo.toml should parse as TOML");

    let level = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(|lints| lints.get("clippy"))
        .and_then(|clippy| clippy.get("manual_clamp"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_clamp is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enables_manual_filter_warn() {
    let mut manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_path.pop();
    manifest_path.pop();
    manifest_path.push("Cargo.toml");

    let manifest_text = std::fs::read_to_string(Path::new(&manifest_path))
        .unwrap_or_else(|_| panic!("failed to read workspace manifest: {}", manifest_path.display()));

    let manifest: Value = toml::from_str(&manifest_text).expect("workspace Cargo.toml should parse as TOML");

    let level = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(|lints| lints.get("clippy"))
        .and_then(|clippy| clippy.get("manual_filter"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_filter is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enforces_manual_flatten_warn() {
    let mut manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_path.pop();
    manifest_path.pop();
    manifest_path.push("Cargo.toml");

    let manifest_text = std::fs::read_to_string(Path::new(&manifest_path))
        .unwrap_or_else(|_| panic!("failed to read workspace manifest: {}", manifest_path.display()));

    let manifest: Value = toml::from_str(&manifest_text).expect("workspace Cargo.toml should parse as TOML");

    let level = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(|lints| lints.get("clippy"))
        .and_then(|clippy| clippy.get("manual_flatten"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_flatten is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enables_manual_map_warn() {
    let mut manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_path.pop();
    manifest_path.pop();
    manifest_path.push("Cargo.toml");

    let manifest_text = std::fs::read_to_string(Path::new(&manifest_path))
        .unwrap_or_else(|_| panic!("failed to read workspace manifest: {}", manifest_path.display()));

    let manifest: Value = toml::from_str(&manifest_text).expect("workspace Cargo.toml should parse as TOML");

    let level = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(|lints| lints.get("clippy"))
        .and_then(|clippy| clippy.get("manual_map"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_map is not configured");

    assert_eq!(level, "warn");
}
