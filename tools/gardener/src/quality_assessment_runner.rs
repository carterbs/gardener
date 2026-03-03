use crate::agent::factory::AdapterFactory;
use crate::agent::AdapterContext;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::output_envelope::{END_MARKER, START_MARKER};
use crate::priority::Priority;
use crate::protocol::AgentTerminal;
use crate::quality_assessment_types::{
    AssessmentPayload, DeficiencyCategory, DomainAssessment, DomainScores, RepoWideAssessment,
    StructuralDeficiency,
};
use crate::quality_complexity_analyzer::analyze_complexity;
use crate::quality_dimension_prompts;
use crate::quality_evidence_bundle::{collect_evidence_bundle, EvidenceBundle};
use crate::quality_file_sampler::{
    format_sampled_files, rank_files_by_complexity, rank_test_files_by_assertions, sample_files,
};
use crate::quality_tree_walker::generate_tree_diagram;
use crate::runtime::ProcessRunner;
use crate::types::AgentKind;
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

pub struct QualityAssessmentConfig {
    pub backend: AgentKind,
    pub model: String,
    pub max_turns: u32,
    pub token_budget: usize,
    pub total_timeout_secs: u64,
}

impl Default for QualityAssessmentConfig {
    fn default() -> Self {
        Self {
            backend: AgentKind::Codex,
            model: "gpt-5-codex".to_string(),
            max_turns: 10,
            token_budget: 80_000,
            total_timeout_secs: 90,
        }
    }
}

/// Run the quality assessment pipeline: collect evidence, invoke agents, parse result.
///
/// When a factory is provided, runs the multi-agent v2 pipeline (parallel dimension agents).
/// When no factory is provided, falls back to deterministic scoring.
/// The boolean indicates whether agents were used (true) or the deterministic fallback (false).
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
            "total_timeout_secs": config.total_timeout_secs,
        }),
    );

    // Step 0: Collect evidence bundle (deterministic pre-computation)
    let bundle = collect_evidence_bundle(repo_path)?;

    // Route to multi-agent pipeline or deterministic fallback
    if let Some(factory) = factory {
        match run_multi_agent_assessment(repo_path, factory, process_runner, config, &bundle) {
            Ok(payload) => {
                append_run_log(
                    "info",
                    "quality_assessment.multi_agent_succeeded",
                    json!({
                        "domain_count": payload.domains.len(),
                        "deficiency_count": payload.deficiencies.len(),
                    }),
                );
                Ok((payload, bundle, true))
            }
            Err(e) => {
                // Multi-agent pipeline failed — fail the assessment entirely (no silent fallback)
                append_run_log(
                    "error",
                    "quality_assessment.multi_agent_failed",
                    json!({ "error": e.to_string() }),
                );
                Err(e)
            }
        }
    } else {
        append_run_log(
            "info",
            "quality_assessment.deterministic_fallback",
            json!({ "reason": "no adapter factory provided" }),
        );
        Ok((deterministic_fallback(&bundle), bundle, false))
    }
}

/// Run the multi-agent v2 assessment pipeline.
///
/// Step 0: Deterministic pre-computation (already done — bundle passed in)
/// Step 1: Domain discovery agent (sequential)
/// Step 2: 9 dimension agents (parallel)
/// Step 3: Synthesizer agent (sequential)
fn run_multi_agent_assessment(
    repo_path: &Path,
    factory: &AdapterFactory,
    process_runner: &dyn ProcessRunner,
    config: &QualityAssessmentConfig,
    bundle: &EvidenceBundle,
) -> Result<AssessmentPayload, GardenerError> {
    let start_time = std::time::Instant::now();

    // --- Step 0b: Additional deterministic pre-computation ---
    let complexity = analyze_complexity(repo_path, &bundle.tree);
    let tree_diagram = generate_tree_diagram(&bundle.tree, 3000);

    // Build shared context
    let manifest_names: Vec<(String, String)> = bundle
        .package_manifests
        .iter()
        .map(|m| {
            let name = m.name.clone().unwrap_or_else(|| m.manifest_type.clone());
            (name, m.path.clone())
        })
        .collect();
    let shared_context = quality_dimension_prompts::build_shared_context(
        &tree_diagram,
        &bundle.tree.language_summary,
        &manifest_names,
    );

    // Pre-sample files for hybrid agents
    let test_assertion_pairs: Vec<(String, usize)> = bundle
        .assertions
        .files
        .iter()
        .map(|f| (f.path.clone(), f.assertion_count))
        .collect();
    let ranked_test_paths = rank_test_files_by_assertions(&test_assertion_pairs);
    let sampled_test_files = sample_files(repo_path, &ranked_test_paths, 500);
    let sampled_test_files_str = format_sampled_files(&sampled_test_files);

    let complexity_pairs: Vec<(String, f64)> = complexity
        .files
        .iter()
        .map(|f| (f.path.clone(), f.complexity_score))
        .collect();
    let ranked_risk_paths = rank_files_by_complexity(&complexity_pairs);
    let sampled_risk_files = sample_files(repo_path, &ranked_risk_paths, 500);
    let sampled_risk_files_str = format_sampled_files(&sampled_risk_files);

    let precompute_elapsed = start_time.elapsed();
    append_run_log(
        "info",
        "quality_assessment.precompute_done",
        json!({
            "elapsed_ms": precompute_elapsed.as_millis() as u64,
            "complexity_files": complexity.summary.total_files,
            "sampled_test_files": sampled_test_files.len(),
            "sampled_risk_files": sampled_risk_files.len(),
        }),
    );

    // --- Step 1: Domain discovery agent ---
    let domain_prompt = quality_dimension_prompts::build_domain_discovery_prompt(&shared_context);
    let domain_output = execute_agent(
        factory,
        process_runner,
        config,
        repo_path,
        "domain-discovery",
        &domain_prompt,
    )?;
    let domain_list = parse_domain_list(&domain_output);

    let discovery_elapsed = start_time.elapsed();
    append_run_log(
        "info",
        "quality_assessment.domain_discovery_done",
        json!({
            "elapsed_ms": discovery_elapsed.as_millis() as u64,
            "domain_list_len": domain_list.len(),
        }),
    );

    // --- Step 2: Build dimension prompts ---
    let test_detector_summary = format_test_detector_summary(bundle);
    let deterministic_test_metrics = format_deterministic_test_metrics(bundle);
    let complexity_metrics = format_complexity_metrics(&complexity);
    let debt_summary = format_debt_summary(bundle);
    let untested_summary = format_untested_summary(bundle);
    let steering_doc_paths = format_steering_doc_paths(bundle);
    let linter_config_paths = format_linter_config_paths(bundle);
    let ci_lint_summary = format_ci_lint_summary(bundle);
    let coverage_summary = format_coverage_summary(bundle);
    let doc_inventory = format_doc_inventory(bundle);

    let dimension_prompts: Vec<(&str, String)> = vec![
        (
            "test_coverage",
            quality_dimension_prompts::build_test_coverage_prompt(
                &shared_context,
                &domain_list,
                &test_detector_summary,
            ),
        ),
        (
            "test_quality",
            quality_dimension_prompts::build_test_quality_prompt(
                &shared_context,
                &domain_list,
                &deterministic_test_metrics,
                &sampled_test_files_str,
            ),
        ),
        (
            "risk_exposure",
            quality_dimension_prompts::build_risk_exposure_prompt(
                &shared_context,
                &domain_list,
                &complexity_metrics,
                &debt_summary,
                &untested_summary,
                &sampled_risk_files_str,
            ),
        ),
        (
            "convention_adherence",
            quality_dimension_prompts::build_convention_adherence_prompt(
                &shared_context,
                &domain_list,
                &steering_doc_paths,
                &linter_config_paths,
            ),
        ),
        (
            "agent_steering",
            quality_dimension_prompts::build_agent_steering_prompt(
                &shared_context,
                &domain_list,
                &steering_doc_paths,
            ),
        ),
        (
            "mechanical_guardrails",
            quality_dimension_prompts::build_mechanical_guardrails_prompt(
                &shared_context,
                &domain_list,
                &ci_lint_summary,
            ),
        ),
        (
            "local_feedback_loop",
            quality_dimension_prompts::build_local_feedback_loop_prompt(
                &shared_context,
                &domain_list,
                &ci_lint_summary,
            ),
        ),
        (
            "coverage_infrastructure",
            quality_dimension_prompts::build_coverage_infrastructure_prompt(
                &shared_context,
                &domain_list,
                &coverage_summary,
            ),
        ),
        (
            "documentation_quality",
            quality_dimension_prompts::build_documentation_quality_prompt(
                &shared_context,
                &domain_list,
                &doc_inventory,
            ),
        ),
    ];

    // --- Step 2b: Execute all dimension agents in parallel ---
    let cache_dir = repo_path.join(".cache/gardener/quality");
    let _ = std::fs::create_dir_all(&cache_dir);

    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let reports: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    std::thread::scope(|s| {
        let handles: Vec<_> = dimension_prompts
            .iter()
            .map(|(dim_name, prompt)| {
                let errors = &errors;
                let reports = &reports;
                let cache_dir = &cache_dir;
                s.spawn(move || {
                    let dim = *dim_name;
                    append_run_log(
                        "info",
                        "quality_assessment.dimension_started",
                        json!({ "dimension": dim }),
                    );

                    match execute_agent(
                        factory,
                        process_runner,
                        config,
                        repo_path,
                        &format!("quality-{dim}"),
                        prompt,
                    ) {
                        Ok(output) => {
                            // Write report to cache
                            let report_path = cache_dir.join(format!("{dim}.md"));
                            let _ = std::fs::write(&report_path, &output);

                            // Also persist to docs/quality-grades/ for seeding agent
                            let stable_dir = repo_path.join("docs").join("quality-grades");
                            let _ = std::fs::create_dir_all(&stable_dir);
                            let stable_path = stable_dir.join(format!("{dim}.md"));
                            let _ = std::fs::write(&stable_path, &output);

                            append_run_log(
                                "info",
                                "quality_assessment.dimension_completed",
                                json!({
                                    "dimension": dim,
                                    "output_len": output.len(),
                                }),
                            );
                            reports
                                .lock()
                                .unwrap()
                                .push((dim.to_string(), output));
                        }
                        Err(e) => {
                            append_run_log(
                                "error",
                                "quality_assessment.dimension_failed",
                                json!({
                                    "dimension": dim,
                                    "error": e.to_string(),
                                }),
                            );
                            errors
                                .lock()
                                .unwrap()
                                .push(format!("{dim}: {e}"));
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            let _ = handle.join();
        }
    });

    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(GardenerError::Process(format!(
            "Dimension agent(s) failed: {}",
            errors.join("; ")
        )));
    }

    let reports = reports.into_inner().unwrap();
    let parallel_elapsed = start_time.elapsed();
    append_run_log(
        "info",
        "quality_assessment.dimensions_all_done",
        json!({
            "elapsed_ms": parallel_elapsed.as_millis() as u64,
            "report_count": reports.len(),
        }),
    );

    // --- Step 3: Synthesizer agent ---
    let report_refs: Vec<(&str, &str)> = reports
        .iter()
        .map(|(name, content)| (name.as_str(), content.as_str()))
        .collect();
    let synthesizer_prompt =
        quality_dimension_prompts::build_synthesizer_prompt(&report_refs, &domain_list);
    let synthesizer_output = execute_agent(
        factory,
        process_runner,
        config,
        repo_path,
        "quality-synthesizer",
        &synthesizer_prompt,
    )?;

    // Parse and validate the final payload
    let payload = parse_assessment_payload(&synthesizer_output)
        .map_err(|e| GardenerError::Process(format!("Synthesizer output parse failed: {e}")))?;
    let validated = validate_payload(payload)
        .map_err(|e| GardenerError::Process(format!("Synthesizer output validation failed: {e}")))?;

    let total_elapsed = start_time.elapsed();
    append_run_log(
        "info",
        "quality_assessment.pipeline_complete",
        json!({
            "total_elapsed_ms": total_elapsed.as_millis() as u64,
            "total_elapsed_secs": total_elapsed.as_secs(),
        }),
    );

    Ok(validated)
}

/// Execute a single agent call and return the text output.
fn execute_agent(
    factory: &AdapterFactory,
    process_runner: &dyn ProcessRunner,
    config: &QualityAssessmentConfig,
    repo_path: &Path,
    agent_id: &str,
    prompt: &str,
) -> Result<String, GardenerError> {
    let adapter = factory.get(config.backend).ok_or_else(|| {
        GardenerError::Process(format!(
            "No adapter available for backend {:?}",
            config.backend
        ))
    })?;

    let output_file = repo_path.join(format!(
        ".cache/gardener/quality-{agent_id}-last-message.json"
    ));
    if let Some(parent) = output_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let context = AdapterContext {
        worker_id: agent_id.to_string(),
        session_id: format!("quality-{agent_id}-session"),
        sandbox_id: format!("quality-{agent_id}-sandbox"),
        model: config.model.clone(),
        cwd: repo_path.to_path_buf(),
        prompt_version: "quality-assessment-v2".to_string(),
        context_manifest_hash: "quality-assessment-context".to_string(),
        output_schema: None,
        output_file: Some(output_file.clone()),
        permissive_mode: true,
        max_turns: Some(config.max_turns),
    };

    let result = adapter.execute(process_runner, &context, prompt, None)?;

    if result.terminal == AgentTerminal::Failure {
        return Err(GardenerError::Process(format!(
            "Agent '{agent_id}' terminated with failure"
        )));
    }

    let output = extract_text_from_result_or_file(&result.payload, &output_file);
    if output.is_empty() || output == "null" {
        return Err(GardenerError::Process(format!(
            "Agent '{agent_id}' produced empty output"
        )));
    }

    Ok(output)
}

/// Parse the domain discovery agent's output into a formatted domain list string.
fn parse_domain_list(raw_output: &str) -> String {
    // Try to extract JSON from the output
    let json_str = if let Some(start) = raw_output.find('{') {
        if let Some(end) = raw_output.rfind('}') {
            &raw_output[start..=end]
        } else {
            raw_output
        }
    } else {
        raw_output
    };

    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
        if let Some(domains) = parsed.get("domains").and_then(|d| d.as_array()) {
            let mut out = String::new();
            for domain in domains {
                let name = domain
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown");
                let description = domain
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("");
                let paths = domain
                    .get("paths")
                    .and_then(|p| p.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                out.push_str(&format!("- **{name}**: {description} (paths: {paths})\n"));
            }
            return out;
        }
    }

    // If parsing fails, return the raw output as-is
    raw_output.to_string()
}

// --- Formatting helpers for dimension prompt inputs ---

fn format_test_detector_summary(bundle: &EvidenceBundle) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Total test files: {}\nTotal source files: {}\n",
        bundle.tree.total_test_files, bundle.tree.total_source_files
    ));
    for (lang, count) in &bundle.tests.summary {
        out.push_str(&format!("- {lang}: {count} test files\n"));
    }
    if !bundle.tests.test_files.is_empty() {
        out.push_str("\nTest files:\n");
        for tf in &bundle.tests.test_files {
            out.push_str(&format!(
                "- {} ({}, {})\n",
                tf.path, tf.language, tf.test_type
            ));
        }
    }
    out
}

fn format_deterministic_test_metrics(bundle: &EvidenceBundle) -> String {
    let mut out = String::new();
    let totals = &bundle.assertions.totals;
    out.push_str(&format!(
        "Total test files analyzed: {}\n\
         Total assertions found: {}\n\
         Average assertions per test file: {:.1}\n",
        totals.total_test_files, totals.total_assertions, totals.avg_assertions_per_file
    ));

    let base_score = (totals.avg_assertions_per_file * 20.0).min(100.0) as u8;
    out.push_str(&format!("\nDeterministic base score: {base_score}/100\n"));

    if !bundle.assertions.files.is_empty() {
        out.push_str("\nPer-file assertion counts (top 10):\n");
        for (i, f) in bundle.assertions.files.iter().take(10).enumerate() {
            out.push_str(&format!(
                "{}. {} — {} assertions\n",
                i + 1,
                f.path,
                f.assertion_count
            ));
        }
    }
    out
}

fn format_complexity_metrics(
    complexity: &crate::quality_complexity_analyzer::ComplexityAnalyzerOutput,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Total files analyzed: {}\nAverage complexity score: {:.1}\n",
        complexity.summary.total_files, complexity.summary.avg_complexity
    ));
    if let Some(ref max_file) = complexity.summary.max_complexity_file {
        out.push_str(&format!("Most complex file: {max_file}\n"));
    }

    out.push_str("\nTop 15 most complex files:\n");
    for (i, f) in complexity.files.iter().take(15).enumerate() {
        out.push_str(&format!(
            "{}. {} — score: {:.1}, branches: {}, nesting: {}, functions: {}, lines: {}\n",
            i + 1,
            f.path,
            f.complexity_score,
            f.branch_count,
            f.max_nesting_depth,
            f.function_count,
            f.line_count,
        ));
    }
    out
}

fn format_debt_summary(bundle: &EvidenceBundle) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Total debt markers: {}\n",
        bundle.debt.total
    ));

    // Count by keyword type
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for marker in &bundle.debt.markers {
        *by_kind.entry(marker.keyword.as_str()).or_default() += 1;
    }
    for (kind, count) in &by_kind {
        out.push_str(&format!("- {kind}: {count}\n"));
    }

    if !bundle.debt.per_file_counts.is_empty() {
        out.push_str("\nFiles with most debt markers:\n");
        let mut sorted: Vec<_> = bundle.debt.per_file_counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (i, (path, count)) in sorted.iter().take(10).enumerate() {
            out.push_str(&format!("{}. {} — {count} markers\n", i + 1, path));
        }
    }
    out
}

fn format_untested_summary(bundle: &EvidenceBundle) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Total source files: {}\nUntested: {}\n",
        bundle.untested.total_count, bundle.untested.untested_count
    ));
    let untested: Vec<_> = bundle
        .untested
        .files
        .iter()
        .filter(|f| !f.has_corresponding_test && !f.has_inline_tests)
        .take(20)
        .collect();
    if !untested.is_empty() {
        out.push_str("\nUntested source files (first 20):\n");
        for f in untested {
            out.push_str(&format!("- {}\n", f.path));
        }
    }
    out
}

fn format_steering_doc_paths(bundle: &EvidenceBundle) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Steering documents found: {}\n",
        bundle.docs.steering_doc_count
    ));
    for doc in &bundle.docs.docs {
        if doc.doc_type == "steering" {
            out.push_str(&format!("- {} ({} lines)\n", doc.path, doc.line_count));
        }
    }
    out
}

fn format_linter_config_paths(bundle: &EvidenceBundle) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Linter detected: {}\n",
        bundle.ci_lint.linters.detected
    ));
    for path in &bundle.ci_lint.linters.files {
        out.push_str(&format!("- {path}\n"));
    }
    out
}

fn format_ci_lint_summary(bundle: &EvidenceBundle) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "CI detected: {} (files: {:?})\n\
         Linters detected: {} (files: {:?})\n\
         Pre-commit detected: {} (files: {:?})\n\
         Coverage thresholds detected: {}\n",
        bundle.ci_lint.ci.detected,
        bundle.ci_lint.ci.files,
        bundle.ci_lint.linters.detected,
        bundle.ci_lint.linters.files,
        bundle.ci_lint.pre_commit.detected,
        bundle.ci_lint.pre_commit.files,
        bundle.ci_lint.coverage_thresholds.detected,
    ));
    out
}

fn format_coverage_summary(bundle: &EvidenceBundle) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Coverage data available: {}\n",
        bundle.coverage.coverage_available
    ));
    if let Some(ref summary) = bundle.coverage.summary {
        out.push_str(&format!("Overall coverage: {:.1}%\n", summary.coverage_percent));
    }
    out.push_str(&format!(
        "Coverage thresholds detected: {}\n",
        bundle.ci_lint.coverage_thresholds.detected
    ));
    out
}

fn format_doc_inventory(bundle: &EvidenceBundle) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Total doc files: {}\n\
         Steering docs: {}\n",
        bundle.docs.total_doc_files, bundle.docs.steering_doc_count,
    ));
    for doc in &bundle.docs.docs {
        out.push_str(&format!("- {} ({})\n", doc.path, doc.doc_type));
    }
    out
}

/// Extract text content from the agent's result payload, falling back to the output file.
///
/// The Codex adapter writes the agent's last message to an output file via `-o`. The event
/// payload's `result` field is often null, so we must read the file as a fallback.
fn extract_text_from_result_or_file(payload: &serde_json::Value, output_file: &Path) -> String {
    let from_payload = extract_text_from_payload(payload);
    if !from_payload.is_empty() && from_payload != "null" {
        return from_payload;
    }

    // Fall back to reading the output file written by the adapter's -o flag
    match std::fs::read_to_string(output_file) {
        Ok(content) if !content.trim().is_empty() => {
            append_run_log(
                "debug",
                "quality_assessment.read_output_file",
                json!({
                    "path": output_file.display().to_string(),
                    "len": content.len(),
                }),
            );
            // The output file may contain JSON with a "message" or text field, or raw text
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                let from_file = extract_text_from_payload(&parsed);
                if !from_file.is_empty() && from_file != "null" {
                    return from_file;
                }
            }
            // Use raw content as-is
            content
        }
        _ => from_payload,
    }
}

/// Extract text content from a JSON payload.
fn extract_text_from_payload(payload: &serde_json::Value) -> String {
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

            let mut dim_rationale = BTreeMap::new();
            dim_rationale.insert(
                "test_coverage".to_string(),
                format!("{tested} of {total} files have corresponding tests."),
            );
            dim_rationale.insert(
                "test_quality".to_string(),
                format!("Assertion density is {assertion_density:.1} per test file."),
            );
            dim_rationale.insert(
                "risk_exposure".to_string(),
                "Defaulted to 50 — deterministic fallback cannot assess risk exposure without agent analysis.".to_string(),
            );
            dim_rationale.insert(
                "convention_adherence".to_string(),
                "Defaulted to 50 — deterministic fallback cannot assess convention adherence without agent analysis.".to_string(),
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
                notes: vec![note],
                dimension_rationale: dim_rationale,
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

    // Repo-wide rationale
    let mut repo_wide_rationale = BTreeMap::new();
    repo_wide_rationale.insert(
        "agent_steering".to_string(),
        format!(
            "{} steering document(s) found (AGENTS.md, CLAUDE.md, etc.). {}",
            steering_doc_count,
            if steering_doc_count == 0 {
                "No guidance for autonomous agents."
            } else if steering_doc_count >= 3 {
                "Good coverage of agent instructions."
            } else {
                "Some steering docs present but could be more comprehensive."
            }
        ),
    );
    repo_wide_rationale.insert(
        "mechanical_guardrails".to_string(),
        format!(
            "Linter: {}. Pre-commit hooks: {}. CI: {}.",
            if has_linter { "detected" } else { "not found" },
            if has_pre_commit { "detected" } else { "not found" },
            if has_ci { "detected" } else { "not found" },
        ),
    );
    repo_wide_rationale.insert(
        "local_feedback_loop".to_string(),
        format!(
            "CI: {}. Tests: {}.",
            if has_ci { "detected" } else { "not found" },
            if has_tests { "present" } else { "none found" },
        ),
    );
    repo_wide_rationale.insert(
        "coverage_infrastructure".to_string(),
        format!(
            "Coverage tooling: {}. Coverage thresholds: {}.",
            if coverage_available { "available" } else { "not detected" },
            if has_coverage_thresholds { "configured" } else { "not configured" },
        ),
    );
    repo_wide_rationale.insert(
        "documentation_quality".to_string(),
        format!(
            "{} documentation file(s) found.",
            total_doc_files,
        ),
    );

    AssessmentPayload {
        domains,
        repo_wide,
        deficiencies,
        domain_file_map,
        primary_gap,
        languages_detected,
        repo_wide_rationale,
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
  "domains": [{"name": "core", "languages": ["Rust"], "scores": {"test_coverage": 80, "test_quality": 70, "risk_exposure": 30, "convention_adherence": 85}, "notes": ["well tested"]}],
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
                notes: vec!["over max".to_string()],
                dimension_rationale: BTreeMap::new(),
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
            repo_wide_rationale: BTreeMap::new(),
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
            repo_wide_rationale: BTreeMap::new(),
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
