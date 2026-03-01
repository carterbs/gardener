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
fn workspace_clippy_lint_configuration_enforces_expect_used_deny() {
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
        .and_then(|clippy| clippy.get("expect_used"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.expect_used is not configured");

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
fn workspace_clippy_lint_configuration_enforces_manual_find_deny() {
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
        .and_then(|clippy| clippy.get("manual_find"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_find is not configured");

    assert_eq!(level, "deny");
}

#[test]
fn workspace_clippy_lint_configuration_enforces_manual_memcpy_deny() {
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
        .and_then(|clippy| clippy.get("manual_memcpy"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_memcpy is not configured");

    assert_eq!(level, "deny");
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
fn workspace_clippy_lint_configuration_enforces_manual_flatten_deny() {
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

    assert_eq!(level, "deny");
}

#[test]
fn workspace_clippy_lint_configuration_enforces_manual_map_deny() {
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

    assert_eq!(level, "deny");
}

#[test]
fn workspace_clippy_lint_configuration_enforces_redundant_static_lifetimes_deny() {
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
        .and_then(|clippy| clippy.get("redundant_static_lifetimes"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.redundant_static_lifetimes is not configured");

    assert_eq!(level, "deny");
}

#[test]
fn check_no_warnings_script_enforces_redundant_static_lifetimes_lint() {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut script_path = manifest_path;
    script_path.pop();
    script_path.pop();
    script_path.push("scripts/check-no-warnings.sh");

    let script_text = std::fs::read_to_string(&script_path)
        .unwrap_or_else(|_| panic!("failed to read script: {}", script_path.display()));

    assert!(
        script_text.contains("clippy::redundant_static_lifetimes"),
        "check-no-warnings.sh should explicitly pass clippy::redundant_static_lifetimes"
    );
}

#[test]
fn workspace_clippy_lint_configuration_enables_manual_try_fold_warn() {
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
        .and_then(|clippy| clippy.get("manual_try_fold"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_try_fold is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enables_manual_unwrap_or_warn() {
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
        .and_then(|clippy| clippy.get("manual_unwrap_or"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_unwrap_or is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enforces_manual_range_contains_deny() {
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
        .and_then(|clippy| clippy.get("manual_range_contains"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_range_contains is not configured");

    assert_eq!(level, "deny");
}

#[test]
fn workspace_clippy_lint_configuration_enforces_manual_non_exhaustive_deny() {
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
        .and_then(|clippy| clippy.get("manual_non_exhaustive"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_non_exhaustive is not configured");

    assert_eq!(level, "deny");
}

#[test]
fn workspace_clippy_lint_configuration_enables_manual_ok_or_warn() {
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
        .and_then(|clippy| clippy.get("manual_ok_or"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_ok_or is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enables_manual_retain_warn() {
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
        .and_then(|clippy| clippy.get("manual_retain"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_retain is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enables_manual_strip_warn() {
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
        .and_then(|clippy| clippy.get("manual_strip"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.manual_strip is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enforces_unnecessary_sort_by_deny() {
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
        .and_then(|clippy| clippy.get("unnecessary_sort_by"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.unnecessary_sort_by is not configured");

    assert_eq!(level, "deny");
}

#[test]
fn workspace_clippy_lint_configuration_enables_needless_borrowed_reference_warn() {
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
        .and_then(|clippy| clippy.get("needless_borrowed_reference"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.needless_borrowed_reference is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enables_needless_question_mark_warn() {
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
        .and_then(|clippy| clippy.get("needless_question_mark"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.needless_question_mark is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enables_needless_update_warn() {
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
        .and_then(|clippy| clippy.get("needless_update"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.needless_update is not configured");

    assert_eq!(level, "warn");
}

#[test]
fn workspace_clippy_lint_configuration_enforces_needless_late_init_deny() {
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
        .and_then(|clippy| clippy.get("needless_late_init"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.needless_late_init is not configured");

    assert_eq!(level, "deny");
}

#[test]
fn workspace_clippy_lint_configuration_enables_unnecessary_lazy_evaluations_deny() {
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
        .and_then(|clippy| clippy.get("unnecessary_lazy_evaluations"))
        .and_then(Value::as_str)
        .expect("workspace.lints.clippy.unnecessary_lazy_evaluations is not configured");

    assert_eq!(level, "deny");
}
