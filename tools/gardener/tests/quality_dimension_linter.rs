//! Linter: ensures QUALITY_DIMENSIONS in the TUI module stays in sync with the
//! dimension keys actually used in quality_assessment_runner.rs.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has parent")
        .parent()
        .expect("grandparent exists")
        .to_path_buf()
}

/// Extract dimension keys from the `dimension_prompts` vec in quality_assessment_runner.rs.
///
/// The source format is:
/// ```ignore
///     let dimension_prompts: Vec<(&str, String)> = vec![
///         (
///             "test_coverage",
///             quality_dimension_prompts::build_test_coverage_prompt(...),
///         ),
///         ...
///     ];
/// ```
///
/// We look for the vec, then find bare `"snake_case_id",` lines inside it.
fn extract_runner_dimensions() -> HashSet<String> {
    let path = repo_root().join("tools/gardener/src/quality_assessment_runner.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    let mut dims = HashSet::new();
    let mut in_vec = false;
    let mut bracket_depth: i32 = 0;

    for line in src.lines() {
        let trimmed = line.trim();

        if !in_vec {
            if trimmed.contains("dimension_prompts") && trimmed.contains("vec![") {
                in_vec = true;
                // Count only square brackets from this point
                bracket_depth =
                    trimmed.matches('[').count() as i32 - trimmed.matches(']').count() as i32;
            }
            continue;
        }

        bracket_depth += trimmed.matches('[').count() as i32;
        bracket_depth -= trimmed.matches(']').count() as i32;

        // Lines like `"test_coverage",` — a bare quoted string followed by comma
        if let Some(after_quote) = trimmed.strip_prefix('"') {
            if let Some(end) = after_quote.find('"') {
                let key = &after_quote[..end];
                let after = after_quote[end + 1..].trim();
                if after.starts_with(',')
                    && !key.is_empty()
                    && key.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                {
                    dims.insert(key.to_string());
                }
            }
        }

        if bracket_depth <= 0 {
            in_vec = false;
        }
    }

    dims
}

/// Extract dimension keys and descriptions from QUALITY_DIMENSIONS in the TUI module.
///
/// The source format is:
/// ```ignore
/// pub const QUALITY_DIMENSIONS: &[(&str, &str)] = &[
///     (
///         "test_coverage",
///         "Measures what fraction of ...",
///     ),
///     ...
/// ];
/// ```
fn extract_tui_dimensions() -> HashMap<String, String> {
    let path = resolve_tui_dimensions_path();
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    let mut dims = HashMap::new();
    let mut in_const = false;
    let mut last_key: Option<String> = None;

    for line in src.lines() {
        let trimmed = line.trim();

        if !in_const {
            if trimmed.contains("QUALITY_DIMENSIONS") && trimmed.contains("const") {
                in_const = true;
            }
            continue;
        }

        if trimmed == "];" {
            break;
        }

        // Key line: `"test_coverage",`
        if let Some(after_quote) = trimmed.strip_prefix('"') {
            if let Some(end) = after_quote.find('"') {
                let candidate = &after_quote[..end];
                let after = after_quote[end + 1..].trim();

                // Dimension key: snake_case followed by comma
                if after.starts_with(',')
                    && !candidate.is_empty()
                    && candidate
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '_')
                {
                    dims.insert(candidate.to_string(), String::new());
                    last_key = Some(candidate.to_string());
                } else if let Some(ref key) = last_key {
                    // Description line: starts with `"` but contains spaces (not a key)
                    if candidate.contains(' ') {
                        if let Some(v) = dims.get_mut(key) {
                            if v.is_empty() {
                                *v = candidate.to_string();
                            }
                        }
                    }
                }
            }
        }
    }

    dims
}

fn resolve_tui_dimensions_path() -> PathBuf {
    let module_path = repo_root().join("tools/gardener/src/tui/quality.rs");
    if module_path.exists() {
        return module_path;
    }
    repo_root().join("tools/gardener/src/tui.rs")
}

#[test]
fn quality_dimensions_match_runner_and_tui() {
    let runner_dims = extract_runner_dimensions();
    let tui_dims = extract_tui_dimensions();

    assert!(
        !runner_dims.is_empty(),
        "failed to extract any dimensions from quality_assessment_runner.rs"
    );
    assert!(
        !tui_dims.is_empty(),
        "failed to extract any dimensions from QUALITY_DIMENSIONS in the TUI module"
    );

    let mut violations = Vec::new();

    for dim in &runner_dims {
        if !tui_dims.contains_key(dim) {
            violations.push(format!(
                "dimension '{dim}' exists in quality_assessment_runner.rs but missing from QUALITY_DIMENSIONS in the TUI module"
            ));
        }
    }

    for dim in tui_dims.keys() {
        if !runner_dims.contains(dim) {
            violations.push(format!(
                "dimension '{dim}' exists in QUALITY_DIMENSIONS in the TUI module but missing from quality_assessment_runner.rs"
            ));
        }
    }

    for (dim, desc) in &tui_dims {
        if desc.trim().is_empty() {
            violations.push(format!(
                "dimension '{dim}' has empty description in QUALITY_DIMENSIONS"
            ));
        }
    }

    if !violations.is_empty() {
        violations.sort();
        panic!(
            "quality dimension linter failed:\n\n{}",
            violations.join("\n")
        );
    }
}

#[test]
fn extract_runner_dimensions_finds_nine() {
    let dims = extract_runner_dimensions();
    assert_eq!(
        dims.len(),
        9,
        "expected 9 runner dimensions, got {}: {:?}",
        dims.len(),
        dims
    );
}

#[test]
fn extract_tui_dimensions_finds_nine() {
    let dims = extract_tui_dimensions();
    assert_eq!(
        dims.len(),
        9,
        "expected 9 TUI dimensions, got {}: {:?}",
        dims.len(),
        dims
    );
}
