use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("crate should live under <repo_root>/tools/gardener")
        .to_path_buf()
}

/// Extract dimension keys from the `dimension_prompts` vec in
/// `quality_assessment_runner.rs`. The canonical pattern is:
///
/// ```text
/// ("dimension_key",
///     quality_dimension_prompts::build_..._prompt(
/// ```
///
/// We look for lines containing a quoted string followed by a comma inside the
/// vec, where the next non-blank line calls a `build_` function.
fn extract_runner_dimensions() -> HashSet<String> {
    let path = repo_root().join("tools/gardener/src/quality_assessment_runner.rs");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));

    let mut dims = HashSet::new();
    let mut in_dimension_prompts = false;
    let mut bracket_depth: i32 = 0;

    for line in source.lines() {
        let trimmed = line.trim();

        // Detect the start of the dimension_prompts vec
        if trimmed.starts_with("let dimension_prompts") && trimmed.contains("vec![") {
            in_dimension_prompts = true;
            // Count opening brackets on this line
            bracket_depth += trimmed.matches('[').count() as i32;
            bracket_depth -= trimmed.matches(']').count() as i32;
            continue;
        }

        if !in_dimension_prompts {
            continue;
        }

        bracket_depth += trimmed.matches('[').count() as i32;
        bracket_depth -= trimmed.matches(']').count() as i32;

        // End of the vec
        if bracket_depth <= 0 {
            break;
        }

        // Look for tuple first-element pattern: "some_key",
        // These are lines like:   "test_coverage",
        if let Some(start) = trimmed.find('"') {
            if let Some(end) = trimmed[start + 1..].find('"') {
                let candidate = &trimmed[start + 1..start + 1 + end];
                // Dimension keys are snake_case identifiers followed by a comma
                let after_quote = trimmed[start + 1 + end + 1..].trim();
                if after_quote.starts_with(',')
                    && candidate.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !candidate.is_empty()
                {
                    dims.insert(candidate.to_string());
                }
            }
        }
    }

    assert!(
        !dims.is_empty(),
        "failed to extract any dimension keys from quality_assessment_runner.rs — \
         has the dimension_prompts vec format changed?"
    );

    dims
}

/// Extract dimension keys and descriptions from the `QUALITY_DIMENSIONS`
/// constant in `tui.rs`. The format is:
///
/// ```text
/// pub const QUALITY_DIMENSIONS: [(&str, &str); N] = [
///     ("dimension_key", "description"),
///     ...
/// ];
/// ```
fn extract_tui_dimensions() -> HashMap<String, String> {
    let path = repo_root().join("tools/gardener/src/tui.rs");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));

    let mut dims = HashMap::new();
    let mut in_const = false;

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed.contains("QUALITY_DIMENSIONS") && trimmed.contains("const") {
            in_const = true;
            continue;
        }

        if !in_const {
            continue;
        }

        // End of the constant (closing ];)
        if trimmed == "];" {
            break;
        }

        // Parse tuple entries like ("key", "description"),
        // Find first quoted string (key) and second quoted string (description)
        if let Some(rest) = trimmed.strip_prefix('(') {
            let rest = rest.trim_start_matches(|c: char| c == '"');
            if let Some(key_end) = rest.find('"') {
                let key = &rest[..key_end];
                let after_key = &rest[key_end + 1..];
                // Skip to next quote to find description
                if let Some(desc_start) = after_key.find('"') {
                    let desc_rest = &after_key[desc_start + 1..];
                    if let Some(desc_end) = desc_rest.find('"') {
                        let desc = &desc_rest[..desc_end];
                        dims.insert(key.to_string(), desc.to_string());
                    }
                }
            }
        }
    }

    assert!(
        !dims.is_empty(),
        "failed to extract any dimensions from QUALITY_DIMENSIONS in tui.rs — \
         has the constant format changed?"
    );

    dims
}

#[test]
fn quality_dimensions_match_runner_and_tui() {
    let runner_dims = extract_runner_dimensions();
    let tui_dims = extract_tui_dimensions();

    let mut violations = Vec::new();

    // Every dimension in the runner must appear in the TUI constant
    let mut sorted_runner: Vec<_> = runner_dims.iter().collect();
    sorted_runner.sort();
    for dim in &sorted_runner {
        if !tui_dims.contains_key(dim.as_str()) {
            violations.push(format!(
                "dimension '{}' exists in quality_assessment_runner.rs but missing from QUALITY_DIMENSIONS in tui.rs",
                dim
            ));
        }
    }

    // Every dimension in the TUI constant must appear in the runner
    let mut sorted_tui: Vec<_> = tui_dims.keys().collect();
    sorted_tui.sort();
    for dim in &sorted_tui {
        if !runner_dims.contains(dim.as_str()) {
            violations.push(format!(
                "dimension '{}' exists in QUALITY_DIMENSIONS in tui.rs but missing from quality_assessment_runner.rs",
                dim
            ));
        }
    }

    // Descriptions must be non-empty
    for dim in &sorted_tui {
        if let Some(desc) = tui_dims.get(dim.as_str()) {
            if desc.trim().is_empty() {
                violations.push(format!(
                    "dimension '{}' has empty description in QUALITY_DIMENSIONS",
                    dim
                ));
            }
        }
    }

    if !violations.is_empty() {
        panic!(
            "quality dimension linter failed:\n\n{}",
            violations.join("\n")
        );
    }
}
