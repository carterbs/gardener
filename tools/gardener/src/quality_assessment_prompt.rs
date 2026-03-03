use crate::quality_evidence_bundle::EvidenceBundle;
use crate::quality_tree_walker::FileEntry;
use std::collections::HashSet;

/// Build the full assessment prompt from an evidence bundle.
pub fn build_assessment_prompt(bundle: &EvidenceBundle) -> String {
    let bundle_json = serde_json::to_string_pretty(bundle).unwrap_or_else(|e| {
        format!("{{\"error\": \"failed to serialize evidence bundle: {e}\"}}")
    });

    format!(
        r#"You are a code quality assessor. Your job is to evaluate how well this repository supports autonomous agent work.

## Evidence Bundle

The following JSON contains the full evidence bundle collected from static analysis of the repository. Use it as your primary data source.

```json
{bundle_json}
```

## Domain Discovery

Using the evidence bundle, identify meaningful domains. A domain is a cohesive area of functionality. Name each domain based on what it does. Use file signatures, package manifests, and directory structure to understand module boundaries. If `domain_hints` is present, use it as a starting point but override based on what you actually find.

## Domain-File Mapping

For each domain, list the source files that belong to it. Every non-test source file must be assigned to exactly one domain. Output this in `domain_file_map`.

## Assessment Instructions

### Per-Domain Scoring (0-100 each)

For each domain, score these dimensions:

- **test_coverage**: What fraction of source files in this domain have corresponding tests? Weight integration and e2e tests more heavily than unit tests. A domain with comprehensive integration tests should score higher than one with only unit tests at the same file coverage ratio.
- **test_quality**: How thorough are the tests? Factor in assertion density (assertions per test file), variety of assertion types, and presence of edge-case testing. Weight integration/e2e tests more heavily.
- **risk_exposure**: How exposed is this domain to bugs and regressions? Factor in TODO/FIXME density, lack of tests for complex files, missing error handling patterns, and the ratio of untested to tested files.
- **convention_adherence**: Does this domain follow the project's conventions? Look for consistent naming, linting compliance, documentation patterns, and adherence to patterns established in steering docs.

### Repo-Wide Scoring (0-100 each)

Score these repository-wide dimensions:

- **agent_steering**: How well does the repo guide autonomous agents? Look for AGENTS.md, CLAUDE.md, CONTRIBUTING.md, and other steering documents.
- **mechanical_guardrails**: Are there automated checks preventing bad code? Look for linters, pre-commit hooks, and CI configuration.
- **local_feedback_loop**: Can a developer (or agent) get fast feedback locally? Look for CI configuration, test runners, and validation commands.
- **coverage_infrastructure**: Is code coverage tracked and enforced? Look for coverage tools, thresholds, and reporting.
- **documentation_quality**: How well-documented is the codebase? Look for READMEs, API docs, inline documentation, and architectural docs.

## Deficiencies

Identify structural deficiencies. Each deficiency must have:
- A description of the gap
- The affected domain (or null for repo-wide)
- A category: one of "CoverageGap", "MissingTooling", "MissingDocumentation", "ConventionViolation", "ObservabilityGap", "FeedbackLoopGap"
- A severity: "P0" (critical), "P1" (important), or "P2" (nice-to-have)
- A suggested task title and details for fixing it

## Primary Gap

Identify the single most impactful gap. This should be one sentence describing the highest-leverage improvement that would most improve this repository's support for autonomous agent work.

## Output Contract

You MUST output your assessment as a JSON object between the markers `<<GARDENER_JSON_START>>` and `<<GARDENER_JSON_END>>`.

The JSON must conform to this exact schema:

```json
{{
  "domains": [
    {{
      "name": "string - domain name",
      "languages": ["string - languages used in this domain"],
      "scores": {{
        "test_coverage": 0,
        "test_quality": 0,
        "risk_exposure": 0,
        "convention_adherence": 0
      }},
      "note": "string - brief assessment note"
    }}
  ],
  "repo_wide": {{
    "agent_steering": 0,
    "mechanical_guardrails": 0,
    "local_feedback_loop": 0,
    "coverage_infrastructure": 0,
    "documentation_quality": 0
  }},
  "deficiencies": [
    {{
      "description": "string",
      "domain": "string or null",
      "category": "CoverageGap | MissingTooling | MissingDocumentation | ConventionViolation | ObservabilityGap | FeedbackLoopGap",
      "severity": "P0 | P1 | P2",
      "suggested_task_title": "string",
      "suggested_task_details": "string"
    }}
  ],
  "domain_file_map": {{
    "domain_name": ["file1.rs", "file2.rs"]
  }},
  "primary_gap": "string - single sentence",
  "languages_detected": ["string"]
}}
```

All score values must be integers between 0 and 100. The `domains` array must not be empty. Every non-test source file must appear in exactly one domain in `domain_file_map`.

<<GARDENER_JSON_START>>
(your JSON here)
<<GARDENER_JSON_END>>"#
    )
}

/// Truncate the evidence bundle for large repos to fit within a token budget.
///
/// Always includes summary-tier data (language_summary, aggregated metrics, docs,
/// CI/lint, coverage summary). Includes per-file details up to the token budget,
/// prioritizing untested files first, then files with debt markers, then the rest.
pub fn truncate_bundle_for_agent(
    bundle: &EvidenceBundle,
    token_budget: usize,
) -> EvidenceBundle {
    // Rough estimate: 4 chars per token
    let char_budget = token_budget * 4;

    // Estimate the size of the full bundle
    let full_json = serde_json::to_string(bundle).unwrap_or_default();
    if full_json.len() <= char_budget {
        return bundle.clone();
    }

    // Build a set of untested file paths for prioritization
    let untested_paths: HashSet<&str> = bundle
        .untested
        .files
        .iter()
        .filter(|f| !f.has_corresponding_test && !f.has_inline_tests)
        .map(|f| f.path.as_str())
        .collect();

    // Build a set of files with debt markers
    let debt_paths: HashSet<&str> = bundle.debt.per_file_counts.keys().map(|k| k.as_str()).collect();

    // Start with a clone and strip per-file detail arrays
    let mut truncated = bundle.clone();
    truncated.truncated = true;

    // Collect all source file entries, prioritized
    let mut priority_files: Vec<(usize, FileEntry, String)> = Vec::new();
    for dir in &bundle.tree.directories {
        for file in &dir.source_files {
            let priority = if untested_paths.contains(file.path.as_str()) {
                0 // highest priority
            } else if debt_paths.contains(file.path.as_str()) {
                1
            } else {
                2
            };
            priority_files.push((priority, file.clone(), dir.path.clone()));
        }
        for file in &dir.test_files {
            priority_files.push((3, file.clone(), dir.path.clone()));
        }
    }
    priority_files.sort_by_key(|(p, _, _)| *p);

    // Estimate the base size (everything minus directory file entries)
    let base_bundle = {
        let mut base = truncated.clone();
        base.tree.directories.clear();
        serde_json::to_string(&base).unwrap_or_default().len()
    };

    let remaining_budget = if char_budget > base_bundle {
        char_budget - base_bundle
    } else {
        0
    };

    // Add files back within budget
    let mut used = 0usize;
    let mut included_dirs: std::collections::BTreeMap<String, (Vec<FileEntry>, Vec<FileEntry>)> =
        std::collections::BTreeMap::new();
    let mut files_included = 0usize;

    for (priority, file, dir_path) in priority_files {
        let file_json = serde_json::to_string(&file).unwrap_or_default();
        let file_cost = file_json.len() + 20; // overhead for array separators, etc.
        if used + file_cost > remaining_budget {
            break;
        }
        used += file_cost;
        files_included += 1;

        let entry = included_dirs
            .entry(dir_path)
            .or_insert_with(|| (Vec::new(), Vec::new()));
        if priority <= 2 {
            entry.0.push(file);
        } else {
            entry.1.push(file);
        }
    }

    truncated.tree.directories = included_dirs
        .into_iter()
        .map(|(path, (source_files, test_files))| {
            crate::quality_tree_walker::DirectoryEntry {
                path,
                source_files,
                test_files,
            }
        })
        .collect();

    truncated.files_included = files_included;
    truncated.files_total = bundle.files_total;

    // Also truncate per-file arrays in other sections proportionally to budget.
    // Use budget-relative caps without artificial minimums to respect low budgets.
    let max_untested_files = (token_budget / 200).max(1);
    if truncated.untested.files.len() > max_untested_files {
        truncated.untested.files.truncate(max_untested_files);
    }

    let max_debt_markers = (token_budget / 100).max(1);
    if truncated.debt.markers.len() > max_debt_markers {
        truncated.debt.markers.truncate(max_debt_markers);
    }

    let max_assertion_files = (token_budget / 200).max(1);
    if truncated.assertions.files.len() > max_assertion_files {
        truncated.assertions.files.truncate(max_assertion_files);
    }

    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality_evidence_bundle::collect_evidence_bundle;
    use tempfile::tempdir;

    #[test]
    fn build_prompt_contains_markers() {
        let dir = tempdir().expect("tempdir");
        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let prompt = build_assessment_prompt(&bundle);
        assert!(prompt.contains("<<GARDENER_JSON_START>>"));
        assert!(prompt.contains("<<GARDENER_JSON_END>>"));
        assert!(prompt.contains("code quality assessor"));
    }

    #[test]
    fn build_prompt_contains_evidence_bundle_json() {
        let dir = tempdir().expect("tempdir");
        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let prompt = build_assessment_prompt(&bundle);
        assert!(prompt.contains("schema_version"));
        assert!(prompt.contains("evidence bundle"));
    }

    #[test]
    fn truncate_small_bundle_is_noop() {
        let dir = tempdir().expect("tempdir");
        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let truncated = truncate_bundle_for_agent(&bundle, 80_000);
        assert!(!truncated.truncated);
    }

    #[test]
    fn truncate_sets_truncated_flag_on_large_bundle() {
        let dir = tempdir().expect("tempdir");
        // Create many files to make the bundle large
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create dir");
        for i in 0..200 {
            std::fs::write(
                src.join(format!("module_{i}.rs")),
                format!("pub fn func_{i}() {{}}\n// TODO: implement\n"),
            )
            .expect("write");
        }
        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        // Use a very small budget to force truncation
        let truncated = truncate_bundle_for_agent(&bundle, 100);
        assert!(truncated.truncated);
        assert!(truncated.files_included <= truncated.files_total);
    }
}
