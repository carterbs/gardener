use crate::agent::factory::AdapterFactory;
use crate::agent::AdapterContext;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::output_envelope::{END_MARKER, START_MARKER};
use crate::priority::Priority;
use crate::protocol::AgentTerminal;
use crate::quality_assessment_prompt::{build_assessment_prompt, truncate_bundle_for_agent};
use crate::quality_assessment_types::{
    AssessmentPayload, DeficiencyCategory, DomainAssessment, DomainScores, RepoWideAssessment,
    StructuralDeficiency,
};
use crate::quality_evidence_bundle::{collect_evidence_bundle, EvidenceBundle};
use crate::runtime::ProcessRunner;
use crate::types::AgentKind;
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

pub struct QualityAssessmentConfig {
    pub backend: AgentKind,
    pub model: String,
    pub max_turns: u32,
    pub token_budget: usize,
}

impl Default for QualityAssessmentConfig {
    fn default() -> Self {
        Self {
            backend: AgentKind::Codex,
            model: "gpt-5-codex".to_string(),
            max_turns: 10,
            token_budget: 80_000,
        }
    }
}

/// Run the quality assessment pipeline: collect evidence, invoke agent, parse result.
///
/// Falls back to deterministic scoring when the agent is unavailable or fails.
/// Result of running the assessment pipeline.
/// The boolean indicates whether the agent was used (true) or the deterministic fallback (false).
pub fn run_assessment(
    repo_path: &Path,
    factory: Option<&AdapterFactory>,
    process_runner: &dyn ProcessRunner,
    config: &QualityAssessmentConfig,
) -> Result<(AssessmentPayload, EvidenceBundle, bool), GardenerError> {
    append_run_log(
        "info",
        "quality_assessment.started",
        json!({
            "repo_path": repo_path.display().to_string(),
            "backend": format!("{:?}", config.backend),
            "model": config.model,
            "token_budget": config.token_budget,
        }),
    );

    // Step 1: Collect evidence bundle
    let bundle = collect_evidence_bundle(repo_path)?;

    // Step 2: Truncate for agent context if needed
    let agent_bundle = truncate_bundle_for_agent(&bundle, config.token_budget);

    // Step 3: Build prompt
    let prompt = build_assessment_prompt(&agent_bundle);

    // Step 4: Try agent execution
    let agent_result = if let Some(factory) = factory {
        try_agent_assessment(factory, process_runner, config, repo_path, &prompt)
    } else {
        append_run_log(
            "info",
            "quality_assessment.no_factory",
            json!({ "reason": "no adapter factory provided, using deterministic fallback" }),
        );
        None
    };

    // Step 5: Use agent result or fall back to deterministic
    let (payload, agent_used) = match agent_result {
        Some(payload) => {
            append_run_log(
                "info",
                "quality_assessment.agent_succeeded",
                json!({
                    "domain_count": payload.domains.len(),
                    "deficiency_count": payload.deficiencies.len(),
                }),
            );
            (payload, true)
        }
        None => {
            append_run_log(
                "info",
                "quality_assessment.deterministic_fallback",
                json!({ "reason": "agent unavailable or failed" }),
            );
            (deterministic_fallback(&bundle), false)
        }
    };

    Ok((payload, bundle, agent_used))
}

/// Attempt to run the assessment through the agent. Returns None on any failure.
fn try_agent_assessment(
    factory: &AdapterFactory,
    process_runner: &dyn ProcessRunner,
    config: &QualityAssessmentConfig,
    repo_path: &Path,
    prompt: &str,
) -> Option<AssessmentPayload> {
    let adapter = factory.get(config.backend)?;

    let context = AdapterContext {
        worker_id: "quality-assessor".to_string(),
        session_id: "quality-assessment-session".to_string(),
        sandbox_id: "quality-assessment-sandbox".to_string(),
        model: config.model.clone(),
        cwd: repo_path.to_path_buf(),
        prompt_version: "quality-assessment-v1".to_string(),
        context_manifest_hash: "quality-assessment-context".to_string(),
        output_schema: None,
        output_file: None,
        permissive_mode: true,
        max_turns: Some(config.max_turns),
    };

    // First attempt
    let result = adapter
        .execute(process_runner, &context, prompt, None)
        .ok()?;

    if result.terminal == AgentTerminal::Failure {
        append_run_log(
            "warn",
            "quality_assessment.agent_failed",
            json!({
                "terminal": "failure",
                "payload": result.payload,
                "diagnostics": result.diagnostics,
            }),
        );
        return None;
    }

    // Try to parse from agent output
    let raw_output = extract_text_from_result(&result.payload);
    match parse_assessment_payload(&raw_output) {
        Ok(payload) => match validate_payload(payload) {
            Ok(validated) => return Some(validated),
            Err(validation_err) => {
                append_run_log(
                    "warn",
                    "quality_assessment.validation_failed",
                    json!({
                        "error": validation_err,
                        "attempt": 1,
                    }),
                );
            }
        },
        Err(parse_err) => {
            append_run_log(
                "warn",
                "quality_assessment.parse_failed",
                json!({
                    "error": parse_err,
                    "attempt": 1,
                    "raw_output_len": raw_output.len(),
                }),
            );
        }
    }

    // Retry once with error context appended
    let retry_prompt = format!(
        "{prompt}\n\n## IMPORTANT: Previous attempt failed\n\
         Your previous response could not be parsed. Make sure your JSON is valid and \
         appears between the <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>> markers. \
         All score values must be integers 0-100. The domains array must not be empty."
    );

    let retry_result = adapter
        .execute(process_runner, &context, &retry_prompt, None)
        .ok()?;

    if retry_result.terminal == AgentTerminal::Failure {
        append_run_log(
            "warn",
            "quality_assessment.retry_agent_failed",
            json!({ "terminal": "failure" }),
        );
        return None;
    }

    let retry_output = extract_text_from_result(&retry_result.payload);
    match parse_assessment_payload(&retry_output) {
        Ok(payload) => match validate_payload(payload) {
            Ok(validated) => Some(validated),
            Err(err) => {
                append_run_log(
                    "error",
                    "quality_assessment.retry_validation_failed",
                    json!({ "error": err }),
                );
                None
            }
        },
        Err(err) => {
            append_run_log(
                "error",
                "quality_assessment.retry_parse_failed",
                json!({ "error": err }),
            );
            None
        }
    }
}

/// Extract text content from the agent's result payload.
///
/// The payload may be a string, or it may be an object with various text fields.
fn extract_text_from_result(payload: &serde_json::Value) -> String {
    // If the payload itself is a string, use it directly
    if let Some(s) = payload.as_str() {
        return s.to_string();
    }

    // Try common fields where agent text output lives
    for key in &["text", "output", "content", "message", "result", "summary"] {
        if let Some(s) = payload.get(key).and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }

    // Last resort: serialize the whole payload
    serde_json::to_string_pretty(payload).unwrap_or_default()
}

/// Parse an AssessmentPayload from raw text that contains JSON between markers.
fn parse_assessment_payload(raw_text: &str) -> Result<AssessmentPayload, String> {
    let start = raw_text
        .rfind(START_MARKER)
        .ok_or_else(|| "missing <<GARDENER_JSON_START>> marker".to_string())?;
    let end = raw_text
        .rfind(END_MARKER)
        .ok_or_else(|| "missing <<GARDENER_JSON_END>> marker".to_string())?;

    if end <= start {
        return Err("end marker appears before start marker".to_string());
    }

    let body_start = start + START_MARKER.len();
    let body = raw_text[body_start..end].trim();

    serde_json::from_str::<AssessmentPayload>(body)
        .map_err(|e| format!("JSON parse error: {e}"))
}

/// Validate and clamp an AssessmentPayload. Fixes out-of-range scores, fills defaults.
fn validate_payload(mut payload: AssessmentPayload) -> Result<AssessmentPayload, String> {
    if payload.domains.is_empty() {
        return Err("domains array is empty".to_string());
    }

    // Clamp all scores to 0-100
    for domain in &mut payload.domains {
        domain.scores.test_coverage = domain.scores.test_coverage.min(100);
        domain.scores.test_quality = domain.scores.test_quality.min(100);
        domain.scores.risk_exposure = domain.scores.risk_exposure.min(100);
        domain.scores.convention_adherence = domain.scores.convention_adherence.min(100);
    }

    payload.repo_wide.agent_steering = payload.repo_wide.agent_steering.min(100);
    payload.repo_wide.mechanical_guardrails = payload.repo_wide.mechanical_guardrails.min(100);
    payload.repo_wide.local_feedback_loop = payload.repo_wide.local_feedback_loop.min(100);
    payload.repo_wide.coverage_infrastructure = payload.repo_wide.coverage_infrastructure.min(100);
    payload.repo_wide.documentation_quality = payload.repo_wide.documentation_quality.min(100);

    // Validate deficiency categories by round-tripping through serde
    for deficiency in &payload.deficiencies {
        let _ = serde_json::to_string(&deficiency.category)
            .map_err(|e| format!("invalid deficiency category: {e}"))?;
    }

    if payload.primary_gap.is_empty() {
        payload.primary_gap = "No specific gap identified.".to_string();
    }

    if payload.languages_detected.is_empty() {
        payload.languages_detected = vec!["Unknown".to_string()];
    }

    Ok(payload)
}

/// Produce a deterministic AssessmentPayload from the evidence bundle when the agent
/// is unavailable.
pub fn deterministic_fallback(bundle: &EvidenceBundle) -> AssessmentPayload {
    // Domain discovery
    let domain_file_map = discover_domains(bundle);

    // Per-domain scoring
    let untested_set: HashSet<&str> = bundle
        .untested
        .files
        .iter()
        .filter(|f| !f.has_corresponding_test && !f.has_inline_tests)
        .map(|f| f.path.as_str())
        .collect();

    let assertion_density = if bundle.assertions.totals.total_test_files > 0 {
        bundle.assertions.totals.avg_assertions_per_file
    } else {
        0.0
    };

    // Collect languages per domain
    let mut domain_languages: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for dir in &bundle.tree.directories {
        for file in &dir.source_files {
            for (domain, files) in &domain_file_map {
                if files.contains(&file.path) {
                    domain_languages
                        .entry(domain.clone())
                        .or_default()
                        .insert(file.language.clone());
                }
            }
        }
    }

    let domains: Vec<DomainAssessment> = domain_file_map
        .iter()
        .map(|(domain_name, files)| {
            let total = files.len();
            let tested = files
                .iter()
                .filter(|f| !untested_set.contains(f.as_str()))
                .count();

            let test_coverage = if total > 0 {
                ((tested as f64 / total as f64) * 100.0) as u8
            } else {
                0
            };

            let test_quality = (assertion_density * 20.0).min(100.0) as u8;

            let languages: Vec<String> = domain_languages
                .get(domain_name)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();

            let note = format!(
                "{tested}/{total} files tested, assertion density {:.1}",
                assertion_density
            );

            DomainAssessment {
                name: domain_name.clone(),
                languages,
                scores: DomainScores {
                    test_coverage,
                    test_quality,
                    risk_exposure: 50,
                    convention_adherence: 50,
                },
                note,
            }
        })
        .collect();

    // Repo-wide scoring
    let steering_doc_count = bundle.docs.steering_doc_count;
    let agent_steering = ((steering_doc_count as u64) * 35).min(100) as u8;

    let has_linter = bundle.ci_lint.linters.detected;
    let has_pre_commit = bundle.ci_lint.pre_commit.detected;
    let has_ci = bundle.ci_lint.ci.detected;
    let mechanical_guardrails = (if has_linter { 40u8 } else { 0 })
        + (if has_pre_commit { 30 } else { 0 })
        + (if has_ci { 30 } else { 0 });

    let has_tests = bundle.tree.total_test_files > 0;
    let local_feedback_loop =
        (if has_ci { 50u8 } else { 0 }) + (if has_tests { 50 } else { 0 });

    let coverage_available = bundle.coverage.coverage_available;
    let has_coverage_thresholds = bundle.ci_lint.coverage_thresholds.detected;
    let coverage_infrastructure = (if coverage_available { 70u8 } else { 0 })
        + (if has_coverage_thresholds { 30 } else { 0 });

    let total_doc_files = bundle.docs.total_doc_files;
    let documentation_quality = ((total_doc_files as u64) * 20).min(100) as u8;

    let repo_wide = RepoWideAssessment {
        agent_steering,
        mechanical_guardrails,
        local_feedback_loop,
        coverage_infrastructure,
        documentation_quality,
    };

    // Build deficiencies
    let mut deficiencies = Vec::new();

    if bundle.untested.untested_count > 0 {
        let pct = if bundle.untested.total_count > 0 {
            (bundle.untested.untested_count as f64 / bundle.untested.total_count as f64) * 100.0
        } else {
            0.0
        };
        deficiencies.push(StructuralDeficiency {
            description: format!(
                "{} of {} source files ({:.0}%) lack test coverage",
                bundle.untested.untested_count, bundle.untested.total_count, pct
            ),
            domain: None,
            category: DeficiencyCategory::CoverageGap,
            severity: if pct > 50.0 {
                Priority::P0
            } else {
                Priority::P1
            },
            suggested_task_title: "Add tests for untested source files".to_string(),
            suggested_task_details: format!(
                "Write tests for the {} source files that currently have no corresponding test files.",
                bundle.untested.untested_count
            ),
        });
    }

    if !has_linter {
        deficiencies.push(StructuralDeficiency {
            description: "No linter configuration detected".to_string(),
            domain: None,
            category: DeficiencyCategory::MissingTooling,
            severity: Priority::P1,
            suggested_task_title: "Add linter configuration".to_string(),
            suggested_task_details: "Configure a linter appropriate for the project's primary language to enforce code style and catch common errors.".to_string(),
        });
    }

    if steering_doc_count == 0 {
        deficiencies.push(StructuralDeficiency {
            description: "No agent steering documents found (AGENTS.md, CLAUDE.md, etc.)".to_string(),
            domain: None,
            category: DeficiencyCategory::MissingDocumentation,
            severity: Priority::P0,
            suggested_task_title: "Add agent steering documentation".to_string(),
            suggested_task_details: "Create an AGENTS.md or CLAUDE.md with instructions for autonomous agents working in this repository.".to_string(),
        });
    }

    if !has_ci {
        deficiencies.push(StructuralDeficiency {
            description: "No CI configuration detected".to_string(),
            domain: None,
            category: DeficiencyCategory::FeedbackLoopGap,
            severity: Priority::P0,
            suggested_task_title: "Set up continuous integration".to_string(),
            suggested_task_details:
                "Add CI configuration (GitHub Actions, etc.) to run tests on every push and PR."
                    .to_string(),
        });
    }

    // Primary gap
    let primary_gap = if steering_doc_count == 0 {
        "Missing agent steering documentation is the highest-leverage gap for supporting autonomous agent work.".to_string()
    } else if !has_ci {
        "Absence of CI configuration prevents automated feedback loops critical for agent-driven development.".to_string()
    } else if bundle.untested.untested_count > bundle.untested.total_count / 2 {
        "More than half of source files lack test coverage, severely limiting an agent's ability to validate changes.".to_string()
    } else if !has_linter {
        "Missing linter configuration means no automated convention enforcement for agent-produced code.".to_string()
    } else {
        "No critical gaps identified; incremental improvements to test coverage and documentation would help.".to_string()
    };

    // Languages detected
    let languages_detected: Vec<String> = bundle
        .tree
        .language_summary
        .keys()
        .cloned()
        .collect();

    AssessmentPayload {
        domains,
        repo_wide,
        deficiencies,
        domain_file_map,
        primary_gap,
        languages_detected,
    }
}

/// Multi-strategy domain discovery from the evidence bundle.
///
/// 1. Package manifests: each package becomes a domain
/// 2. Directory clustering: group by second-level dir
/// 3. Flat repos: single "repository" domain
/// 4. Leftover files go to "other"
fn discover_domains(bundle: &EvidenceBundle) -> BTreeMap<String, Vec<String>> {
    let mut domain_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut assigned: HashSet<String> = HashSet::new();

    // Collect all source file paths
    let all_source_files: Vec<String> = bundle
        .tree
        .directories
        .iter()
        .flat_map(|d| d.source_files.iter().map(|f| f.path.clone()))
        .collect();

    // Strategy 1: Package manifests - each package becomes a domain.
    // Process non-root manifests first so sub-packages claim their files
    // before the root manifest gets the leftovers.
    if bundle.package_manifests.len() > 1 {
        // Separate root vs non-root manifests
        let (root_manifests, sub_manifests): (Vec<_>, Vec<_>) =
            bundle.package_manifests.iter().partition(|m| {
                let pkg_dir = Path::new(&m.path)
                    .parent()
                    .and_then(|p| p.to_str())
                    .unwrap_or("");
                pkg_dir.is_empty()
            });

        // Process sub-packages first
        for manifest in sub_manifests.iter().chain(root_manifests.iter()) {
            let domain_name = manifest
                .name
                .as_ref()
                .map(|n| n.clone())
                .unwrap_or_else(|| {
                    let p = Path::new(&manifest.path);
                    p.parent()
                        .and_then(|pp| pp.to_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("root")
                        .to_string()
                });

            let pkg_dir = Path::new(&manifest.path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("");

            for file_path in &all_source_files {
                if assigned.contains(file_path) {
                    continue;
                }
                let matches = if pkg_dir.is_empty() {
                    // Root manifest — only gets files not claimed by sub-packages
                    true
                } else {
                    // Use boundary-aware check: file must be directly under pkg_dir/
                    file_path.starts_with(&format!("{pkg_dir}/"))
                        || file_path == pkg_dir
                };
                if matches {
                    domain_map
                        .entry(domain_name.clone())
                        .or_default()
                        .push(file_path.clone());
                    assigned.insert(file_path.clone());
                }
            }
        }
    }

    // Strategy 2: Directory clustering for unassigned files
    let skip_roots: HashSet<&str> = ["src", "lib", "app", "pkg"].iter().copied().collect();

    for file_path in &all_source_files {
        if assigned.contains(file_path) {
            continue;
        }

        let parts: Vec<&str> = file_path.split('/').collect();
        let domain_name = if parts.len() >= 2 {
            let first = parts[0];
            if skip_roots.contains(first) && parts.len() >= 3 {
                // Skip src/lib/app/pkg roots, use the next level
                parts[1].to_string()
            } else {
                first.to_string()
            }
        } else {
            // Flat file at root level
            "repository".to_string()
        };

        domain_map
            .entry(domain_name)
            .or_default()
            .push(file_path.clone());
        assigned.insert(file_path.clone());
    }

    // Strategy 3: If we ended up with zero domains, create a single "repository" domain
    if domain_map.is_empty() && !all_source_files.is_empty() {
        let unassigned: Vec<String> = all_source_files
            .iter()
            .filter(|f| !assigned.contains(f.as_str()))
            .cloned()
            .collect();
        domain_map.insert("repository".to_string(), unassigned);
        return domain_map;
    }

    // If there are still unassigned files (shouldn't happen, but safety net)
    let remaining: Vec<String> = all_source_files
        .iter()
        .filter(|f| !assigned.contains(f.as_str()))
        .cloned()
        .collect();
    if !remaining.is_empty() {
        domain_map
            .entry("other".to_string())
            .or_default()
            .extend(remaining);
    }

    domain_map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality_evidence_bundle::collect_evidence_bundle;
    use tempfile::tempdir;

    #[test]
    fn deterministic_fallback_empty_repo() {
        let dir = tempdir().expect("tempdir");
        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let payload = deterministic_fallback(&bundle);
        // Empty repo should have no domains (no source files)
        assert!(payload.domains.is_empty() || payload.domains[0].name == "repository");
        assert!(payload.repo_wide.agent_steering <= 100);
    }

    #[test]
    fn deterministic_fallback_with_source_files() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create dir");
        std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(src.join("lib.rs"), "pub fn hello() {}\n").expect("write");

        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let payload = deterministic_fallback(&bundle);

        assert!(!payload.domains.is_empty());
        assert!(!payload.domain_file_map.is_empty());
        assert!(!payload.languages_detected.is_empty());
    }

    #[test]
    fn deterministic_fallback_scores_are_bounded() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create dir");
        for i in 0..10 {
            std::fs::write(
                src.join(format!("mod_{i}.rs")),
                format!("pub fn f_{i}() {{}}\n"),
            )
            .expect("write");
        }

        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let payload = deterministic_fallback(&bundle);

        for domain in &payload.domains {
            assert!(domain.scores.test_coverage <= 100);
            assert!(domain.scores.test_quality <= 100);
            assert!(domain.scores.risk_exposure <= 100);
            assert!(domain.scores.convention_adherence <= 100);
        }
        assert!(payload.repo_wide.agent_steering <= 100);
        assert!(payload.repo_wide.mechanical_guardrails <= 100);
        assert!(payload.repo_wide.local_feedback_loop <= 100);
        assert!(payload.repo_wide.coverage_infrastructure <= 100);
        assert!(payload.repo_wide.documentation_quality <= 100);
    }

    #[test]
    fn deterministic_fallback_detects_missing_ci() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create dir");
        std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write");

        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let payload = deterministic_fallback(&bundle);

        // Should have a deficiency for missing CI
        assert!(payload
            .deficiencies
            .iter()
            .any(|d| matches!(d.category, DeficiencyCategory::FeedbackLoopGap)));
    }

    #[test]
    fn deterministic_fallback_detects_missing_steering_docs() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create dir");
        std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write");

        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let payload = deterministic_fallback(&bundle);

        assert!(payload
            .deficiencies
            .iter()
            .any(|d| matches!(d.category, DeficiencyCategory::MissingDocumentation)));
    }

    #[test]
    fn discover_domains_uses_directory_clustering() {
        let dir = tempdir().expect("tempdir");
        let auth = dir.path().join("src/auth");
        let api = dir.path().join("src/api");
        std::fs::create_dir_all(&auth).expect("create dir");
        std::fs::create_dir_all(&api).expect("create dir");
        std::fs::write(auth.join("login.rs"), "pub fn login() {}\n").expect("write");
        std::fs::write(api.join("routes.rs"), "pub fn routes() {}\n").expect("write");

        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let domains = discover_domains(&bundle);

        // src/ is skipped, so we should see "auth" and "api" domains
        assert!(domains.contains_key("auth") || domains.contains_key("api"));
    }

    #[test]
    fn discover_domains_flat_repo() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").expect("write");

        let bundle = collect_evidence_bundle(dir.path()).expect("bundle");
        let domains = discover_domains(&bundle);

        assert!(domains.contains_key("repository"));
    }

    #[test]
    fn parse_assessment_payload_extracts_json() {
        let raw = r#"Some text before
<<GARDENER_JSON_START>>
{
  "domains": [{"name": "core", "languages": ["Rust"], "scores": {"test_coverage": 80, "test_quality": 70, "risk_exposure": 30, "convention_adherence": 85}, "note": "well tested"}],
  "repo_wide": {"agent_steering": 60, "mechanical_guardrails": 80, "local_feedback_loop": 90, "coverage_infrastructure": 50, "documentation_quality": 40},
  "deficiencies": [],
  "domain_file_map": {"core": ["src/main.rs"]},
  "primary_gap": "Missing coverage thresholds",
  "languages_detected": ["Rust"]
}
<<GARDENER_JSON_END>>
Some text after"#;

        let payload = parse_assessment_payload(raw).expect("should parse");
        assert_eq!(payload.domains.len(), 1);
        assert_eq!(payload.domains[0].name, "core");
        assert_eq!(payload.domains[0].scores.test_coverage, 80);
    }

    #[test]
    fn parse_assessment_payload_fails_without_markers() {
        let raw = r#"{"domains": [], "repo_wide": {}}"#;
        assert!(parse_assessment_payload(raw).is_err());
    }

    #[test]
    fn validate_payload_clamps_scores() {
        let payload = AssessmentPayload {
            domains: vec![DomainAssessment {
                name: "test".to_string(),
                languages: vec!["Rust".to_string()],
                scores: DomainScores {
                    test_coverage: 255,
                    test_quality: 200,
                    risk_exposure: 150,
                    convention_adherence: 100,
                },
                note: "over max".to_string(),
            }],
            repo_wide: RepoWideAssessment {
                agent_steering: 200,
                mechanical_guardrails: 100,
                local_feedback_loop: 100,
                coverage_infrastructure: 100,
                documentation_quality: 100,
            },
            deficiencies: vec![],
            domain_file_map: BTreeMap::new(),
            primary_gap: "gap".to_string(),
            languages_detected: vec!["Rust".to_string()],
        };

        let validated = validate_payload(payload).expect("should validate");
        assert_eq!(validated.domains[0].scores.test_coverage, 100);
        assert_eq!(validated.domains[0].scores.test_quality, 100);
        assert_eq!(validated.domains[0].scores.risk_exposure, 100);
        assert_eq!(validated.repo_wide.agent_steering, 100);
    }

    #[test]
    fn validate_payload_rejects_empty_domains() {
        let payload = AssessmentPayload {
            domains: vec![],
            repo_wide: RepoWideAssessment {
                agent_steering: 50,
                mechanical_guardrails: 50,
                local_feedback_loop: 50,
                coverage_infrastructure: 50,
                documentation_quality: 50,
            },
            deficiencies: vec![],
            domain_file_map: BTreeMap::new(),
            primary_gap: "gap".to_string(),
            languages_detected: vec!["Rust".to_string()],
        };

        assert!(validate_payload(payload).is_err());
    }

    #[test]
    fn run_assessment_uses_deterministic_fallback_without_factory() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create dir");
        std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write");

        let config = QualityAssessmentConfig::default();
        let runner = crate::runtime::FakeProcessRunner::default();

        let (payload, bundle, _agent_used) =
            run_assessment(dir.path(), None, &runner, &config).expect("should succeed");

        assert!(!payload.domains.is_empty());
        assert!(!bundle.tree.directories.is_empty());
    }

    #[test]
    fn default_config_values() {
        let config = QualityAssessmentConfig::default();
        assert_eq!(config.backend, AgentKind::Codex);
        assert_eq!(config.model, "gpt-5-codex");
        assert_eq!(config.max_turns, 10);
        assert_eq!(config.token_budget, 80_000);
    }
}
