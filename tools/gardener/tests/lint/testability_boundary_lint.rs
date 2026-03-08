use std::path::Path;

use crate::testability_boundary::{
    module_path_for, read_repo_file, strip_comments_and_literals, BoundaryManifest,
    InstrumentationPolicy, Role,
};

const BANNED_UNIT_CORE_PATTERNS: &[&str] = &[
    "std::process::",
    "Command::new(",
    "std::fs::",
    "fs::",
    "std::env::",
    "current_dir(",
    "var_os(",
    "var(",
    "append_run_log(",
    "structured_fallback_line(",
    "crossterm::",
    "expectrl::",
    "enable_raw_mode(",
    "disable_raw_mode(",
    "execute!(",
    "WorktreeClient::",
    "ProductionProcessRunner::",
    "current_run_id(",
    "current_run_log_path(",
];

#[test]
fn manifest_is_fail_closed_for_in_scope_files() {
    let manifest = BoundaryManifest::load();
    let mut missing = Vec::new();

    for path in manifest.in_scope_paths() {
        if manifest.allowlisted_paths.contains(&path) {
            continue;
        }
        if manifest.entry_for_path(&path).is_none() {
            missing.push(path);
        }
    }

    assert!(
        missing.is_empty(),
        "testability boundary manifest is fail-closed; add entries or allowlist these files:\n{}",
        missing.join("\n")
    );
}

#[test]
fn manifest_entries_are_well_formed_and_in_scope() {
    let manifest = BoundaryManifest::load();
    let in_scope = manifest.in_scope_paths().into_iter().collect::<Vec<_>>();
    let in_scope_set = in_scope.iter().cloned().collect::<std::collections::BTreeSet<_>>();
    let mut failures = Vec::new();

    for allowlisted in &manifest.allowlisted_paths {
        if !in_scope_set.contains(allowlisted) {
            failures.push(format!("allowlisted path is outside scope or missing: {allowlisted}"));
        }
    }

    for entry in &manifest.entries {
        if !in_scope_set.contains(&entry.path) {
            failures.push(format!("manifest entry is outside scope or missing: {}", entry.path));
        }

        if entry.role == Role::BoundaryOrchestration {
            if entry.owning_tests.is_empty() {
                failures.push(format!("boundary file has no owning tests: {}", entry.path));
            }
            if entry.boundary_modes.is_empty() {
                failures.push(format!("boundary file has no boundary_modes: {}", entry.path));
            }
            for target in &entry.owning_tests {
                let test_path = manifest
                    .repo_root
                    .join("tools/gardener/tests")
                    .join(format!("{target}.rs"));
                if !test_path.exists() {
                    failures.push(format!(
                        "boundary file {} references missing owning test target {}",
                        entry.path, target
                    ));
                }
            }
        } else {
            if !entry.owning_tests.is_empty() {
                failures.push(format!(
                    "unit-core file {} must not declare owning_tests",
                    entry.path
                ));
            }
            if !entry.boundary_modes.is_empty() {
                failures.push(format!(
                    "unit-core file {} must not declare boundary_modes",
                    entry.path
                ));
            }
            if entry.instrumentation == InstrumentationPolicy::Required {
                failures.push(format!(
                    "unit-core file {} must not require runtime instrumentation",
                    entry.path
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "testability boundary manifest validation failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn unit_core_files_do_not_depend_on_boundary_side_effects() {
    let manifest = BoundaryManifest::load();
    let boundary_modules = manifest
        .boundary_entries()
        .map(|entry| module_path_for(&entry.path))
        .collect::<Vec<_>>();
    let mut failures = Vec::new();

    for entry in manifest.unit_core_entries() {
        let source = read_repo_file(&entry.path);
        let sanitized = strip_comments_and_literals(&source);
        let stem = Path::new(&entry.path)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("<unknown>");

        for pattern in BANNED_UNIT_CORE_PATTERNS {
            if sanitized.contains(pattern) {
                failures.push(format!(
                    "{} contains banned unit-core side effect pattern `{pattern}`",
                    entry.path
                ));
            }
        }

        if sanitized.contains("use super::terminal")
            || sanitized.contains("super::terminal::")
            || sanitized.contains("crate::tui::terminal::")
        {
            failures.push(format!(
                "{} depends on boundary terminal module",
                entry.path
            ));
        }

        for boundary_module in &boundary_modules {
            if boundary_module.ends_with(&format!("::{stem}")) {
                continue;
            }
            if sanitized.contains(boundary_module) {
                failures.push(format!(
                    "{} depends on boundary module {}",
                    entry.path, boundary_module
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "unit-core purity enforcement failed:\n{}",
        failures.join("\n")
    );
}
