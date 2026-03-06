use std::collections::BTreeMap;

use crate::output_envelope::{END_MARKER, START_MARKER};

/// Shared context included in every dimension agent's prompt.
///
/// Formats the tree diagram, language summary, and package manifest names
/// into a clean readable block that each agent gets prepended.
pub fn build_shared_context(
    tree_diagram: &str,
    language_summary: &BTreeMap<String, usize>,
    manifest_names: &[(String, String)],
) -> String {
    let mut out = String::with_capacity(tree_diagram.len() + 512);

    out.push_str("## Repository Structure\n\n```\n");
    out.push_str(tree_diagram);
    if !tree_diagram.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("```\n\n");

    out.push_str("## Language Breakdown\n\n");
    for (lang, count) in language_summary {
        out.push_str(&format!("- {lang}: {count} files\n"));
    }
    out.push('\n');

    if !manifest_names.is_empty() {
        out.push_str("## Package Manifests\n\n");
        for (name, path) in manifest_names {
            out.push_str(&format!("- **{name}** (`{path}`)\n"));
        }
        out.push('\n');
    }

    out
}

/// Domain discovery agent prompt.
pub fn build_domain_discovery_prompt(shared_context: &str) -> String {
    format!(
        r#"You are a domain discovery agent. Your job is to identify meaningful functional domains in a repository.

{shared_context}

## Task

Analyze the repository structure above and identify cohesive domains. A domain is a functional area of the codebase — not just a directory, but a logical unit with a clear purpose.

Use package manifests as the primary signal for domain boundaries. Use directory clustering as a secondary signal.

Keep it simple: most repos have 2-8 domains. Don't over-segment.

## Output Format

Respond with a JSON object (and nothing else):

```json
{{
  "domains": [
    {{
      "name": "short-kebab-case-name",
      "paths": ["dir/prefix1/", "dir/prefix2/"],
      "description": "One sentence describing what this domain does."
    }}
  ]
}}
```

Rules:
- Every source file should belong to exactly one domain via its path prefix.
- Domain names should describe function, not structure (e.g., "quality-assessment" not "src-quality").
- Paths are directory prefixes — a file matches if its path starts with any listed prefix.
- If in doubt, fewer larger domains are better than many tiny ones."#
    )
}

/// Test coverage dimension prompt.
pub fn build_test_coverage_prompt(
    shared_context: &str,
    domain_list: &str,
    test_detector_summary: &str,
) -> String {
    format!(
        r#"You are a specialist quality assessor focused on **test coverage**.

{shared_context}

## Domains

{domain_list}

## Test Detection Summary

{test_detector_summary}

## Scoring Rubric (0-100)

Score each domain and the repo overall on what fraction of meaningful source files have corresponding tests.

- **90-100**: Near-complete coverage. All critical paths tested, integration and e2e tests present.
- **70-89**: Good coverage with some gaps. Most source files have tests.
- **50-69**: Moderate coverage. Many files untested, but core functionality covered.
- **30-49**: Sparse coverage. Only a few areas have tests.
- **0-29**: Minimal or no test coverage.

Weight integration and e2e tests more heavily than unit tests. A domain with comprehensive integration tests should score higher than one with only unit tests at the same file coverage ratio.

{DIMENSION_OUTPUT_FORMAT}
"#,
        DIMENSION_OUTPUT_FORMAT = dimension_output_format("Test Coverage"),
    )
}

/// Test quality dimension prompt (hybrid — deterministic base + agent spot-check).
pub fn build_test_quality_prompt(
    shared_context: &str,
    domain_list: &str,
    deterministic_metrics: &str,
    sampled_test_files: &str,
) -> String {
    format!(
        r#"You are a specialist quality assessor focused on **test quality**.

{shared_context}

## Domains

{domain_list}

## Deterministic Analysis

A deterministic analysis has already been performed. The base scores are provided below.

{deterministic_metrics}

Your job is to validate or adjust these scores based on reading the sampled files. You may adjust each score by up to ±15 points from the deterministic base.

## Sampled Test Files

{sampled_test_files}

## Scoring Rubric (0-100)

Assess how thorough and well-structured tests are.

- **90-100**: Tests are substantive, cover edge cases, use meaningful assertions, have good isolation.
- **70-89**: Tests are solid but may miss some edge cases or rely on trivial assertions.
- **50-69**: Tests exist but are superficial — happy path only, weak assertions.
- **30-49**: Tests are pro-forma. Low assertion density, no edge cases.
- **0-29**: Tests are absent or effectively meaningless.

Key signals: assertion density, variety of assertion types, edge-case coverage, test isolation, meaningful test names, absence of commented-out tests.

{DIMENSION_OUTPUT_FORMAT}
"#,
        DIMENSION_OUTPUT_FORMAT = dimension_output_format("Test Quality"),
    )
}

/// Risk exposure dimension prompt (hybrid — deterministic complexity + agent review).
pub fn build_risk_exposure_prompt(
    shared_context: &str,
    domain_list: &str,
    complexity_metrics: &str,
    debt_summary: &str,
    untested_summary: &str,
    sampled_risk_files: &str,
) -> String {
    format!(
        r#"You are a specialist quality assessor focused on **risk exposure**.

{shared_context}

## Domains

{domain_list}

## Deterministic Analysis

A deterministic analysis has already been performed. The base scores are provided below.

{complexity_metrics}

Your job is to validate or adjust these scores based on reading the sampled files. You may adjust each score by up to ±15 points from the deterministic base.

## Debt Markers

{debt_summary}

## Untested Files

{untested_summary}

## Sampled High-Risk Files

{sampled_risk_files}

## Scoring Rubric (0-100)

Score how exposed each domain is to bugs and regressions. **Lower scores mean higher risk.**

- **90-100**: Low risk. Well-tested, low complexity, minimal debt.
- **70-89**: Moderate risk. Some complex untested code, but manageable.
- **50-69**: Elevated risk. Significant untested complexity or debt clusters.
- **30-49**: High risk. Critical paths are untested, high complexity, debt accumulation.
- **0-29**: Severe risk. Complex code with no tests, extensive debt, missing error handling.

Key signals: untested-to-tested ratio in complex files, debt marker density, nesting depth, error handling gaps, missing validation at boundaries.

{DIMENSION_OUTPUT_FORMAT}
"#,
        DIMENSION_OUTPUT_FORMAT = dimension_output_format("Risk Exposure"),
    )
}

/// Convention adherence dimension prompt.
pub fn build_convention_adherence_prompt(
    shared_context: &str,
    domain_list: &str,
    steering_doc_paths: &str,
    linter_config_paths: &str,
) -> String {
    format!(
        r#"You are a specialist quality assessor focused on **convention adherence**.

{shared_context}

## Domains

{domain_list}

## Steering Document Paths

{steering_doc_paths}

## Linter Configuration Paths

{linter_config_paths}

## Scoring Rubric (0-100)

Score how well the codebase follows its own stated conventions.

- **90-100**: Consistent style throughout. Linters configured and passing. Naming conventions followed.
- **70-89**: Mostly consistent with minor deviations. Linters present but not all rules enforced.
- **50-69**: Conventions exist but compliance is spotty. Some areas follow patterns, others don't.
- **30-49**: Conventions are poorly defined or widely ignored. Inconsistent naming, formatting.
- **0-29**: No discernible conventions. Each file does its own thing.

Read the steering docs and linter configs to understand what conventions the project expects. Then spot-check source files to see if those conventions are actually followed.

{DIMENSION_OUTPUT_FORMAT}
"#,
        DIMENSION_OUTPUT_FORMAT = dimension_output_format("Convention Adherence"),
    )
}

/// Agent steering dimension prompt.
pub fn build_agent_steering_prompt(
    shared_context: &str,
    domain_list: &str,
    steering_doc_paths: &str,
) -> String {
    format!(
        r#"You are a specialist quality assessor focused on **agent steering**.

{shared_context}

## Domains

{domain_list}

## Steering Document Paths

{steering_doc_paths}

## Scoring Rubric (Research-Based)

Evaluate the steering documentation against these criteria, informed by recent research on what makes steering docs effective:

### Specificity over comprehensiveness
Does the file contain concrete, actionable directives ("run tests with `make test`", "use `uv` not `pip`") vs vague overviews? Less prescriptive, specific guidance outperforms comprehensive context files.

### Signal-to-noise ratio
Is the file concise (<300 lines ideally, <500 max) or bloated with LLM-generated boilerplate? Files accumulate content without systematic revision — maintenance debt reduces effectiveness.

### Architecture pointers
Does it describe module boundaries and where to find things? 72.6% of effective steering files include architecture pointers.

### Testing/build commands
Are the exact commands specified? 75% of effective steering files include these. Agents need to know *exactly* how to run tests, not just that tests exist.

### Progressive disclosure
Does it link to deeper docs rather than inlining everything? Good files are navigational — they tell you *where* to look, not *everything*.

### Anti-patterns to penalize
- Auto-generated overviews (LLM-generated context files *reduced* success rates by ~3% while increasing costs 20%+)
- Duplicated README content
- Walls of text without structure
- Vague guidance ("follow best practices")

### Cross-tool compatibility
Is the guidance tool-agnostic or locked to one specific agent? The best files work across Claude, Codex, Cursor, and others.

## Score Anchors

- **90-100**: Concise, specific, actionable. Contains architecture map, build/test commands, conventions. Links to deeper docs. Under 300 lines.
- **70-89**: Good content but some bloat, or missing one key category (architecture, testing, conventions).
- **50-69**: Present but generic. Overviews without actionable directives. Or over-long with buried signals.
- **30-49**: Minimal — exists but provides little value beyond "this is a Python project."
- **0-29**: Missing or effectively empty.

Read each steering document and evaluate it against this rubric. Be rigorous — most files should score 40-75, not 90+.

{DIMENSION_OUTPUT_FORMAT}
"#,
        DIMENSION_OUTPUT_FORMAT = dimension_output_format("Agent Steering"),
    )
}

/// Mechanical guardrails dimension prompt.
pub fn build_mechanical_guardrails_prompt(
    shared_context: &str,
    domain_list: &str,
    ci_lint_summary: &str,
) -> String {
    format!(
        r#"You are a specialist quality assessor focused on **mechanical guardrails**.

{shared_context}

## Domains

{domain_list}

## CI/Lint Detection Summary

{ci_lint_summary}

## Scoring Rubric (0-100)

Score the breadth and enforcement level of automated checks preventing bad code from landing.

- **90-100**: Comprehensive guardrails. Linters, formatters, type checkers, pre-commit hooks, CI gates all present and enforced.
- **70-89**: Good coverage. Most categories present but one or two gaps (e.g., no pre-commit hooks, or linter not enforced in CI).
- **50-69**: Partial guardrails. CI exists but limited checks. Or linters configured but not enforced.
- **30-49**: Minimal. Only basic CI or only a linter, not both. No pre-commit hooks.
- **0-29**: No automated guardrails detected.

Check for: linter configs, pre-commit hooks, CI pipeline configs, type checking configs, formatters, security scanners.

{DIMENSION_OUTPUT_FORMAT}
"#,
        DIMENSION_OUTPUT_FORMAT = dimension_output_format("Mechanical Guardrails"),
    )
}

/// Local feedback loop dimension prompt.
pub fn build_local_feedback_loop_prompt(
    shared_context: &str,
    domain_list: &str,
    ci_config_summary: &str,
) -> String {
    format!(
        r#"You are a specialist quality assessor focused on **local feedback loop**.

{shared_context}

## Domains

{domain_list}

## CI Configuration Summary

{ci_config_summary}

## Scoring Rubric (0-100)

Score how quickly and easily a developer (or agent) can validate changes locally.

- **90-100**: Fast, comprehensive local feedback. Test suite runs in seconds, watch mode available, Makefile/justfile with clear targets, CI commands runnable locally.
- **70-89**: Good feedback loop. Tests run reasonably fast, build commands documented, most CI checks reproducible locally.
- **50-69**: Functional but slow or incomplete. Tests take minutes, or some CI checks can't be run locally.
- **30-49**: Difficult local feedback. No clear entry point for running tests, or feedback cycle is very slow.
- **0-29**: No local feedback path. Must push to CI to validate anything.

Check for: Makefile/justfile/package.json scripts, test runner configs, watch-mode configs, documented dev workflows, CI commands that can be run locally.

{DIMENSION_OUTPUT_FORMAT}
"#,
        DIMENSION_OUTPUT_FORMAT = dimension_output_format("Local Feedback Loop"),
    )
}

/// Coverage infrastructure dimension prompt.
pub fn build_coverage_infrastructure_prompt(
    shared_context: &str,
    domain_list: &str,
    coverage_summary: &str,
) -> String {
    format!(
        r#"You are a specialist quality assessor focused on **coverage infrastructure**.

{shared_context}

## Domains

{domain_list}

## Coverage Detection Summary

{coverage_summary}

## Scoring Rubric (0-100)

Score whether code coverage is measured, reported, and enforced.

- **90-100**: Coverage fully instrumented. Thresholds enforced in CI, coverage reports generated and visible, coverage badges present.
- **70-89**: Coverage measured and reported, but thresholds not enforced or not visible in PR workflow.
- **50-69**: Coverage tools configured but not integrated into CI. Or coverage measured but no thresholds.
- **30-49**: Minimal coverage setup. Tool config exists but not wired into the workflow.
- **0-29**: No coverage infrastructure detected.

Check for: coverage tool configs (tarpaulin.toml, .coveragerc, jest coverage settings), CI coverage steps, coverage thresholds/gates, coverage badges or reporting integrations.

{DIMENSION_OUTPUT_FORMAT}
"#,
        DIMENSION_OUTPUT_FORMAT = dimension_output_format("Coverage Infrastructure"),
    )
}

/// Documentation quality dimension prompt.
pub fn build_documentation_quality_prompt(
    shared_context: &str,
    domain_list: &str,
    doc_inventory: &str,
) -> String {
    format!(
        r#"You are a specialist quality assessor focused on **documentation quality**.

{shared_context}

## Domains

{domain_list}

## Documentation Inventory

{doc_inventory}

## Scoring Rubric (0-100)

Score how well-documented the codebase is.

- **90-100**: Excellent docs. Clear README, API docs, architectural docs, inline docs on complex functions, doc generation configured.
- **70-89**: Good docs with some gaps. README is solid, some API docs, but inline docs sparse or architectural overview missing.
- **50-69**: Basic docs. README exists but is thin. Little to no API or architectural documentation.
- **30-49**: Minimal docs. README is a stub or outdated. No other documentation.
- **0-29**: No meaningful documentation.

Check for: README quality and completeness, API docs, architectural docs, inline documentation density in key files, doc generation configs (rustdoc, godoc, typedoc).

{DIMENSION_OUTPUT_FORMAT}
"#,
        DIMENSION_OUTPUT_FORMAT = dimension_output_format("Documentation Quality"),
    )
}

/// Synthesizer prompt — compiles all dimension reports into the final AssessmentPayload.
pub fn build_synthesizer_prompt(dimension_reports: &[(&str, &str)], domain_list: &str) -> String {
    let mut reports_section = String::new();
    for (dimension, report) in dimension_reports {
        reports_section.push_str(&format!("### {dimension}\n\n{report}\n\n---\n\n"));
    }

    format!(
        r#"You are the quality assessment synthesizer. Your job is to compile individual dimension reports into a single structured assessment.

## Domains

{domain_list}

## Dimension Reports

{reports_section}

## Task

Compile the dimension reports above into a single JSON assessment payload.

For each domain mentioned in the dimension reports, extract the per-domain scores for:
- `test_coverage`, `test_quality`, `risk_exposure`, `convention_adherence`

For the repo-wide assessment, extract:
- `agent_steering`, `mechanical_guardrails`, `local_feedback_loop`, `coverage_infrastructure`, `documentation_quality`

Also compile:
- `domain_file_map`: map each domain name to its list of source file paths (from the domain list above)
- `primary_gap`: identify the single most impactful gap from across all dimension reports (one sentence)
- `languages_detected`: list all languages mentioned across reports
- `repo_wide_rationale`: for each repo-wide dimension key (agent_steering, mechanical_guardrails, local_feedback_loop, coverage_infrastructure, documentation_quality), write 2-4 sentences explaining what's strong, what's weak, and why this score. The rationale should help someone understand the score without reading the full dimension report.
- `deficiencies`: merge deficiency lists from all dimension reports. Each must have: description (2-3 sentences explaining the gap AND why it hurts agent performance — do not compress to one line), domain (or null for repo-wide), category (one of "CoverageGap", "MissingTooling", "MissingDocumentation", "ConventionViolation", "ObservabilityGap", "FeedbackLoopGap"), severity ("P0", "P1", or "P2"), suggested_task_title (imperative verb phrase), and suggested_task_details (2-3 sentences with concrete steps).

For each domain, also provide `dimension_rationale`: a map from each score key (test_coverage, test_quality, risk_exposure, convention_adherence) to a 1-2 sentence explanation of that score.

IMPORTANT: The `description` field must explain both WHAT the gap is and WHY it degrades autonomous agent performance. Example: "29 source files lack test coverage including runtime boundary files (main.rs, phase_cli.rs, merge_loop.rs). Without tests on these entry points, agents cannot validate that orchestration changes don't break startup, phase transitions, or merge workflows. This forces manual verification of every change touching the runtime boundary."

## Output Contract

You MUST output the final assessment as a JSON object between these markers:

{START_MARKER}
(your JSON here)
{END_MARKER}

The JSON must conform to this exact schema:

```json
{{
  "domains": [
    {{
      "name": "string",
      "languages": ["string"],
      "scores": {{
        "test_coverage": 0,
        "test_quality": 0,
        "risk_exposure": 0,
        "convention_adherence": 0
      }},
      "notes": ["string - 2 to 4 bullets mixing strengths and weaknesses"],
      "dimension_rationale": {{
        "test_coverage": "string - 1-2 sentences",
        "test_quality": "string - 1-2 sentences",
        "risk_exposure": "string - 1-2 sentences",
        "convention_adherence": "string - 1-2 sentences"
      }}
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
  "repo_wide_rationale": {{
    "agent_steering": "string - 2-4 sentences",
    "mechanical_guardrails": "string - 2-4 sentences",
    "local_feedback_loop": "string - 2-4 sentences",
    "coverage_infrastructure": "string - 2-4 sentences",
    "documentation_quality": "string - 2-4 sentences"
  }},
  "primary_gap": "string - single sentence",
  "languages_detected": ["string"]
}}
```

All score values must be integers between 0 and 100. The `domains` array must not be empty."#,
    )
}

/// Standard output format instructions appended to each dimension prompt.
fn dimension_output_format(dimension_name: &str) -> String {
    format!(
        r#"## Output Format

Produce your assessment in this exact format:

## {dimension_name} Assessment

### Repo-Wide Score: [0-100]
[Brief justification — 1-3 sentences]

### Per-Domain Scores
- domain_name: [0-100] - [brief justification]
- domain_name: [0-100] - [brief justification]

### Key Findings
- [finding 1]
- [finding 2]
- [finding 3]

### Deficiencies

For each deficiency, provide 2-3 bullets explaining:
1. What the gap is (be specific — name files, tools, or patterns)
2. Why this hurts autonomous agent performance (what fails, what's slower, what's riskier)
3. What the fix looks like (concrete action, not vague advice)

Format:
- **[category | severity]** Brief title
  - What: [specific description of the gap]
  - Agent impact: [how this concretely degrades agent performance — failed runs, wasted turns, missed regressions, etc.]
  - Fix: [actionable remediation]

Categories: CoverageGap, MissingTooling, MissingDocumentation, ConventionViolation, ObservabilityGap, FeedbackLoopGap
Severities: P0 (critical), P1 (important), P2 (nice to have)

Scores must be integers 0-100."#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_shared_context() -> String {
        let mut langs = BTreeMap::new();
        langs.insert("Rust".to_string(), 95);
        langs.insert("Shell".to_string(), 12);
        let manifests = vec![(
            "gardener".to_string(),
            "tools/gardener/Cargo.toml".to_string(),
        )];
        build_shared_context(".\n├── src/\n└── tests/\n", &langs, &manifests)
    }

    #[test]
    fn shared_context_is_non_empty() {
        let ctx = sample_shared_context();
        assert!(!ctx.is_empty());
        assert!(ctx.contains("Repository Structure"));
        assert!(ctx.contains("Rust: 95 files"));
        assert!(ctx.contains("Shell: 12 files"));
        assert!(ctx.contains("gardener"));
    }

    #[test]
    fn domain_discovery_includes_json_output_instructions() {
        let ctx = sample_shared_context();
        let prompt = build_domain_discovery_prompt(&ctx);
        assert!(prompt.contains("\"domains\""));
        assert!(prompt.contains("\"paths\""));
        assert!(prompt.contains("JSON"));
        assert!(prompt.contains("domain discovery"));
    }

    #[test]
    fn synthesizer_includes_json_markers() {
        let reports = vec![
            ("Test Coverage", "Score: 75"),
            ("Agent Steering", "Score: 60"),
        ];
        let prompt = build_synthesizer_prompt(&reports, "domain-a, domain-b");
        assert!(prompt.contains(START_MARKER));
        assert!(prompt.contains(END_MARKER));
        assert!(prompt.contains("synthesizer"));
    }

    #[test]
    fn each_dimension_prompt_includes_name_and_scoring() {
        let ctx = sample_shared_context();
        let domains = "- core: tools/gardener/src/";

        let prompts: Vec<(&str, String)> = vec![
            (
                "test coverage",
                build_test_coverage_prompt(&ctx, domains, "summary"),
            ),
            (
                "test quality",
                build_test_quality_prompt(&ctx, domains, "metrics", "files"),
            ),
            (
                "risk exposure",
                build_risk_exposure_prompt(
                    &ctx,
                    domains,
                    "complexity",
                    "debt",
                    "untested",
                    "files",
                ),
            ),
            (
                "convention adherence",
                build_convention_adherence_prompt(&ctx, domains, "docs", "linters"),
            ),
            (
                "agent steering",
                build_agent_steering_prompt(&ctx, domains, "docs"),
            ),
            (
                "mechanical guardrails",
                build_mechanical_guardrails_prompt(&ctx, domains, "ci"),
            ),
            (
                "local feedback loop",
                build_local_feedback_loop_prompt(&ctx, domains, "ci"),
            ),
            (
                "coverage infrastructure",
                build_coverage_infrastructure_prompt(&ctx, domains, "coverage"),
            ),
            (
                "documentation quality",
                build_documentation_quality_prompt(&ctx, domains, "docs"),
            ),
        ];

        for (name, prompt) in &prompts {
            assert!(
                prompt.contains("specialist quality assessor"),
                "{name} prompt missing role statement"
            );
            assert!(
                prompt.contains("0-100"),
                "{name} prompt missing scoring range"
            );
            assert!(
                prompt.contains("Scoring Rubric"),
                "{name} prompt missing rubric"
            );
            assert!(
                prompt.contains("Output Format"),
                "{name} prompt missing output format"
            );
        }
    }

    #[test]
    fn agent_steering_includes_research_rubric_keywords() {
        let ctx = sample_shared_context();
        let prompt = build_agent_steering_prompt(&ctx, "domains", "docs");

        // Research-based rubric keywords
        assert!(prompt.contains("Specificity over comprehensiveness"));
        assert!(prompt.contains("Signal-to-noise ratio"));
        assert!(prompt.contains("72.6%"));
        assert!(prompt.contains("Architecture pointers"));
        assert!(prompt.contains("Progressive disclosure"));
        assert!(prompt.contains("Anti-patterns"));
        assert!(prompt.contains("Cross-tool compatibility"));

        // Score anchors
        assert!(prompt.contains("90-100"));
        assert!(prompt.contains("70-89"));
        assert!(prompt.contains("50-69"));
        assert!(prompt.contains("30-49"));
        assert!(prompt.contains("0-29"));
    }

    #[test]
    fn hybrid_prompts_include_adjustment_instructions() {
        let ctx = sample_shared_context();
        let domains = "- core";

        let test_quality = build_test_quality_prompt(&ctx, domains, "metrics", "files");
        assert!(test_quality.contains("deterministic analysis has already been performed"));
        assert!(test_quality.contains("±15 points"));

        let risk = build_risk_exposure_prompt(&ctx, domains, "cx", "debt", "untested", "files");
        assert!(risk.contains("deterministic analysis has already been performed"));
        assert!(risk.contains("±15 points"));
    }

    #[test]
    fn synthesizer_includes_all_dimension_reports() {
        let reports = vec![
            ("Test Coverage", "Coverage report content"),
            ("Test Quality", "Quality report content"),
            ("Risk Exposure", "Risk report content"),
        ];
        let prompt = build_synthesizer_prompt(&reports, "domain-a");
        assert!(prompt.contains("Coverage report content"));
        assert!(prompt.contains("Quality report content"));
        assert!(prompt.contains("Risk report content"));
        assert!(prompt.contains("domain_file_map"));
        assert!(prompt.contains("primary_gap"));
        assert!(prompt.contains("languages_detected"));
        assert!(prompt.contains("deficiencies"));
    }

    #[test]
    fn shared_context_handles_empty_manifests() {
        let langs = BTreeMap::new();
        let ctx = build_shared_context(".\n", &langs, &[]);
        assert!(ctx.contains("Repository Structure"));
        assert!(!ctx.contains("Package Manifests"));
    }
}
