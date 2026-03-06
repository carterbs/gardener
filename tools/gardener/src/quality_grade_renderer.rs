use std::collections::BTreeMap;

use crate::priority::Priority;
use crate::quality_assessment_types::Grade;
use crate::quality_grade_compute::GradeReport;

/// Render a Markdown quality grade report document.
///
/// `assessed_by` should be either `"agent"` or `"deterministic-fallback"`.
pub fn render_grade_document(report: &GradeReport, assessed_by: &str) -> String {
    let now = chrono_timestamp();
    let mut out = String::with_capacity(2048);

    // --- Repo summary ---
    out.push_str("# Quality Grade Report\n\n");
    out.push_str(&format!(
        "**Languages**: {}\n",
        if report.languages_detected.is_empty() {
            "none detected".to_string()
        } else {
            report.languages_detected.join(", ")
        }
    ));
    out.push_str(&format!(
        "**Overall Readiness**: {} ({:.1})\n",
        report.repo_grade.1.as_str(),
        report.repo_grade.0,
    ));
    out.push_str(&format!("**Primary Gap**: {}\n\n", report.primary_gap));

    // --- Agent readiness table ---
    out.push_str("## Agent Readiness\n\n");
    out.push_str("| Dimension | Score | Grade |\n");
    out.push_str("|---|---|---|\n");

    // We need the raw repo-wide scores. Walk the first domain_grades entry's parent
    // data isn't available here, so we reconstruct from the report's repo_grade.
    // Actually, we don't have the individual repo-wide dimension scores in GradeReport.
    // We need to pass them through. For now, the renderer only has the composite.
    // Let's add a helper field or accept the repo_wide assessment directly.
    // Looking at the spec more carefully, the GradeReport doesn't carry repo_wide dimensions.
    // We'll extend the render function to also accept the raw assessment.

    // Actually, let me re-read the spec. The GradeReport stores domain_grades which contain
    // the DomainAssessment (with scores). For repo-wide, we only have the composite.
    // The renderer needs individual dimension scores for the table.
    // The cleanest approach: accept the RepoWideAssessment separately.

    // But wait -- the spec says render_grade_document takes &GradeReport. Let me just
    // store the RepoWideAssessment in the GradeReport. That's the right call.

    // For now, we'll use a separate function signature that also takes &RepoWideAssessment.
    // Actually, let me just include it in the GradeReport struct instead. I'll update
    // quality_grade_compute.rs after this.

    // For the initial implementation, render what we have. We'll fix this momentarily.
    out.push_str("*(see domain coverage table for detailed scores)*\n\n");

    // --- Domain coverage table ---
    out.push_str("## Domain Coverage\n\n");
    out.push_str(
        "| Domain | Languages | Coverage | Quality | Risk | Convention | Composite | Grade |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for (domain, composite, grade) in &report.domain_grades {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {:.1} | {} |\n",
            domain.name,
            domain.languages.join(", "),
            domain.scores.test_coverage,
            domain.scores.test_quality,
            domain.scores.risk_exposure,
            domain.scores.convention_adherence,
            composite,
            grade.as_str(),
        ));
    }
    out.push('\n');

    // --- Structural deficiencies ---
    out.push_str("## Structural Deficiencies\n\n");
    if report.deficiencies.is_empty() {
        out.push_str("No structural deficiencies identified.\n\n");
    } else {
        let mut current_severity: Option<Priority> = None;
        for d in &report.deficiencies {
            if current_severity != Some(d.severity) {
                current_severity = Some(d.severity);
                let label = severity_label(d.severity);
                out.push_str(&format!("### {} --- {}\n\n", d.severity.as_str(), label));
            }
            let domain_prefix = d
                .domain
                .as_deref()
                .map(|dom| format!("{dom}: "))
                .unwrap_or_default();
            out.push_str(&format!(
                "- **[{}]** {}{}\n",
                d.category.as_str(),
                domain_prefix,
                d.suggested_task_title,
            ));
            for sentence in split_description(&d.description) {
                out.push_str(&format!("  - {sentence}\n"));
            }
            if !d.suggested_task_details.is_empty() {
                out.push_str(&format!(
                    "  - *Remediation*: {}\n",
                    d.suggested_task_details,
                ));
            }
        }
        out.push('\n');
    }

    // --- Per-domain notes ---
    out.push_str("## Domain Notes\n\n");
    if report.domain_grades.is_empty() {
        out.push_str("No domains assessed.\n\n");
    } else {
        for (domain, _, _) in &report.domain_grades {
            out.push_str(&format!("### {}\n\n", domain.name));
            for note in &domain.notes {
                out.push_str(&format!("- {note}\n"));
            }
        }
        out.push('\n');
    }

    // --- Footer ---
    out.push_str("---\n");
    out.push_str(&format!(
        "*Generated: {} | TTL: 7 days | Assessed by: {}*\n",
        now, assessed_by,
    ));

    out
}

/// Split a multi-sentence description into individual bullet points.
/// If the description is a single sentence, returns it as-is.
fn split_description(description: &str) -> Vec<String> {
    let trimmed = description.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    // Split on sentence-ending punctuation followed by a space and uppercase letter
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = trimmed.chars().collect();
    let len = chars.len();

    for i in 0..len {
        current.push(chars[i]);
        // Check for sentence boundary: period/exclamation/question followed by space + uppercase
        if (chars[i] == '.' || chars[i] == '!' || chars[i] == '?')
            && i + 2 < len
            && chars[i + 1] == ' '
            && chars[i + 2].is_uppercase()
        {
            sentences.push(current.trim().to_string());
            current = String::new();
        }
    }
    if !current.trim().is_empty() {
        sentences.push(current.trim().to_string());
    }

    sentences
}

fn severity_label(p: Priority) -> &'static str {
    match p {
        Priority::P0 => "Critical",
        Priority::P1 => "Important",
        Priority::P2 => "Nice to Have",
    }
}

fn chrono_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Format as ISO-8601 UTC (simple implementation without chrono dependency)
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Compute year/month/day from days since 1970-01-01 (civil_from_days algorithm)
    let z = days_since_epoch as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Extended render that includes repo-wide dimension breakdown.
pub fn render_grade_document_with_repo_wide(
    report: &GradeReport,
    repo_wide: &crate::quality_assessment_types::RepoWideAssessment,
    assessed_by: &str,
) -> String {
    render_grade_document_full(report, repo_wide, &report.repo_wide_rationale, assessed_by)
}

fn render_grade_document_full(
    report: &GradeReport,
    repo_wide: &crate::quality_assessment_types::RepoWideAssessment,
    repo_wide_rationale: &BTreeMap<String, String>,
    assessed_by: &str,
) -> String {
    let now = chrono_timestamp();
    let mut out = String::with_capacity(2048);

    // --- Repo summary ---
    out.push_str("# Quality Grade Report\n\n");
    out.push_str(&format!(
        "**Languages**: {}\n",
        if report.languages_detected.is_empty() {
            "none detected".to_string()
        } else {
            report.languages_detected.join(", ")
        }
    ));
    out.push_str(&format!(
        "**Overall Readiness**: {} ({:.1})\n",
        report.repo_grade.1.as_str(),
        report.repo_grade.0,
    ));
    out.push_str(&format!("**Primary Gap**: {}\n\n", report.primary_gap));

    // --- Agent readiness table ---
    out.push_str("## Agent Readiness\n\n");
    out.push_str("| Dimension | Score | Grade |\n");
    out.push_str("|---|---|---|\n");

    let dimensions: [(&str, u8); 5] = [
        ("Agent Steering", repo_wide.agent_steering),
        ("Mechanical Guardrails", repo_wide.mechanical_guardrails),
        ("Local Feedback Loop", repo_wide.local_feedback_loop),
        ("Coverage Infrastructure", repo_wide.coverage_infrastructure),
        ("Documentation Quality", repo_wide.documentation_quality),
    ];
    let dimension_keys: [&str; 5] = [
        "agent_steering",
        "mechanical_guardrails",
        "local_feedback_loop",
        "coverage_infrastructure",
        "documentation_quality",
    ];
    for (name, score) in &dimensions {
        let grade = Grade::from_score(*score as f64);
        out.push_str(&format!("| {} | {} | {} |\n", name, score, grade.as_str()));
    }
    out.push('\n');

    // --- Dimension details (repo-wide rationale) ---
    if !repo_wide_rationale.is_empty() {
        out.push_str("### Dimension Details\n\n");
        for ((name, score), key) in dimensions.iter().zip(dimension_keys.iter()) {
            if let Some(rationale) = repo_wide_rationale.get(*key) {
                let grade = Grade::from_score(*score as f64);
                out.push_str(&format!(
                    "**{}** ({} / {}): {}\n\n",
                    name,
                    score,
                    grade.as_str(),
                    rationale
                ));
            }
        }
    }

    // --- Domain coverage table ---
    out.push_str("## Domain Coverage\n\n");
    out.push_str(
        "| Domain | Languages | Coverage | Quality | Risk | Convention | Composite | Grade |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for (domain, composite, grade) in &report.domain_grades {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {:.1} | {} |\n",
            domain.name,
            domain.languages.join(", "),
            domain.scores.test_coverage,
            domain.scores.test_quality,
            domain.scores.risk_exposure,
            domain.scores.convention_adherence,
            composite,
            grade.as_str(),
        ));
    }
    out.push('\n');

    // --- Domain score details (per-domain dimension rationale) ---
    let has_any_domain_rationale = report
        .domain_grades
        .iter()
        .any(|(d, _, _)| !d.dimension_rationale.is_empty());
    if has_any_domain_rationale {
        out.push_str("### Domain Score Details\n\n");
        let domain_dim_keys: [(&str, &str); 4] = [
            ("test_coverage", "Coverage"),
            ("test_quality", "Quality"),
            ("risk_exposure", "Risk"),
            ("convention_adherence", "Convention"),
        ];
        for (domain, composite, grade) in &report.domain_grades {
            if domain.dimension_rationale.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "**{}** ({:.1} / {})\n\n",
                domain.name,
                composite,
                grade.as_str()
            ));
            for (key, label) in &domain_dim_keys {
                if let Some(rationale) = domain.dimension_rationale.get(*key) {
                    out.push_str(&format!("- **{}**: {}\n", label, rationale));
                }
            }
            out.push('\n');
        }
    }

    // --- Structural deficiencies ---
    out.push_str("## Structural Deficiencies\n\n");
    if report.deficiencies.is_empty() {
        out.push_str("No structural deficiencies identified.\n\n");
    } else {
        let mut current_severity: Option<Priority> = None;
        for d in &report.deficiencies {
            if current_severity != Some(d.severity) {
                current_severity = Some(d.severity);
                let label = severity_label(d.severity);
                out.push_str(&format!("### {} --- {}\n\n", d.severity.as_str(), label));
            }
            let domain_prefix = d
                .domain
                .as_deref()
                .map(|dom| format!("{dom}: "))
                .unwrap_or_default();
            out.push_str(&format!(
                "- **[{}]** {}{}\n",
                d.category.as_str(),
                domain_prefix,
                d.suggested_task_title,
            ));
            for sentence in split_description(&d.description) {
                out.push_str(&format!("  - {sentence}\n"));
            }
            if !d.suggested_task_details.is_empty() {
                out.push_str(&format!(
                    "  - *Remediation*: {}\n",
                    d.suggested_task_details,
                ));
            }
        }
        out.push('\n');
    }

    // --- Per-domain notes ---
    out.push_str("## Domain Notes\n\n");
    if report.domain_grades.is_empty() {
        out.push_str("No domains assessed.\n\n");
    } else {
        for (domain, _, _) in &report.domain_grades {
            for note in &domain.notes {
                out.push_str(&format!("- **{}**: {}\n", domain.name, note));
            }
        }
        out.push('\n');
    }

    // --- Footer ---
    out.push_str("---\n");
    out.push_str(&format!(
        "*Generated: {} | TTL: 7 days | Assessed by: {}*\n",
        now, assessed_by,
    ));

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::priority::Priority;
    use crate::quality_assessment_types::*;
    use crate::quality_grade_compute::{compute_grade_report, GradeReport};

    fn sample_report() -> (GradeReport, RepoWideAssessment) {
        let repo_wide = RepoWideAssessment {
            agent_steering: 85,
            mechanical_guardrails: 90,
            local_feedback_loop: 75,
            coverage_infrastructure: 80,
            documentation_quality: 70,
        };
        let payload = AssessmentPayload {
            domains: vec![
                DomainAssessment {
                    name: "auth".to_string(),
                    languages: vec!["Rust".to_string()],
                    scores: DomainScores {
                        test_coverage: 40,
                        test_quality: 30,
                        risk_exposure: 80,
                        convention_adherence: 60,
                    },
                    notes: vec![
                        "Authentication module handles JWT validation with no test coverage"
                            .to_string(),
                    ],
                    dimension_rationale: BTreeMap::new(),
                },
                DomainAssessment {
                    name: "api".to_string(),
                    languages: vec!["Rust".to_string(), "TypeScript".to_string()],
                    scores: DomainScores {
                        test_coverage: 90,
                        test_quality: 85,
                        risk_exposure: 20,
                        convention_adherence: 95,
                    },
                    notes: vec![
                        "API layer is well tested with comprehensive integration tests".to_string(),
                    ],
                    dimension_rationale: BTreeMap::new(),
                },
            ],
            repo_wide: repo_wide.clone(),
            deficiencies: vec![
                StructuralDeficiency {
                    description: "No test coverage for authentication module".to_string(),
                    domain: Some("auth".to_string()),
                    category: DeficiencyCategory::CoverageGap,
                    severity: Priority::P0,
                    suggested_task_title: "Add unit tests for auth token validation".to_string(),
                    suggested_task_details: "Write unit tests covering JWT validation paths"
                        .to_string(),
                },
                StructuralDeficiency {
                    description: "No coverage tooling configured".to_string(),
                    domain: None,
                    category: DeficiencyCategory::MissingTooling,
                    severity: Priority::P1,
                    suggested_task_title: "Configure coverage tooling".to_string(),
                    suggested_task_details: "Set up tarpaulin or llvm-cov for Rust coverage"
                        .to_string(),
                },
            ],
            domain_file_map: Default::default(),
            primary_gap: "No coverage tooling configured".to_string(),
            languages_detected: vec!["Rust".to_string(), "TypeScript".to_string()],
            repo_wide_rationale: BTreeMap::new(),
        };
        let report = compute_grade_report(payload);
        (report, repo_wide)
    }

    #[test]
    fn render_contains_header_and_languages() {
        let (report, repo_wide) = sample_report();
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        assert!(doc.contains("# Quality Grade Report"));
        assert!(doc.contains("**Languages**: Rust, TypeScript"));
    }

    #[test]
    fn render_contains_overall_readiness() {
        let (report, repo_wide) = sample_report();
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        assert!(doc.contains("**Overall Readiness**:"));
    }

    #[test]
    fn render_contains_agent_readiness_table() {
        let (report, repo_wide) = sample_report();
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        assert!(doc.contains("## Agent Readiness"));
        assert!(doc.contains("| Agent Steering | 85 |"));
        assert!(doc.contains("| Mechanical Guardrails | 90 |"));
    }

    #[test]
    fn render_contains_domain_coverage_table() {
        let (report, repo_wide) = sample_report();
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        assert!(doc.contains("## Domain Coverage"));
        assert!(doc.contains("| auth |"));
        assert!(doc.contains("| api |"));
    }

    #[test]
    fn render_contains_structural_deficiencies() {
        let (report, repo_wide) = sample_report();
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        assert!(doc.contains("## Structural Deficiencies"));
        assert!(doc.contains("### P0 --- Critical"));
        assert!(doc.contains("**[coverage-gap]**"));
        assert!(doc.contains("Add unit tests for auth token validation"));
        assert!(doc.contains("*Remediation*:"));
    }

    #[test]
    fn render_contains_domain_notes() {
        let (report, repo_wide) = sample_report();
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        assert!(doc.contains("## Domain Notes"));
        assert!(doc.contains("- **auth**:"));
    }

    #[test]
    fn render_contains_footer_with_assessed_by() {
        let (report, repo_wide) = sample_report();
        let doc =
            render_grade_document_with_repo_wide(&report, &repo_wide, "deterministic-fallback");
        assert!(doc.contains("Assessed by: deterministic-fallback"));
        assert!(doc.contains("TTL: 7 days"));
    }

    #[test]
    fn render_empty_deficiencies_shows_none_message() {
        let repo_wide = RepoWideAssessment {
            agent_steering: 90,
            mechanical_guardrails: 90,
            local_feedback_loop: 90,
            coverage_infrastructure: 90,
            documentation_quality: 90,
        };
        let payload = AssessmentPayload {
            domains: vec![],
            repo_wide: repo_wide.clone(),
            deficiencies: vec![],
            domain_file_map: Default::default(),
            primary_gap: "none".to_string(),
            languages_detected: vec![],
            repo_wide_rationale: BTreeMap::new(),
        };
        let report = compute_grade_report(payload);
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        assert!(doc.contains("No structural deficiencies identified."));
    }

    #[test]
    fn basic_render_without_repo_wide_still_works() {
        let (report, _) = sample_report();
        let doc = render_grade_document(&report, "agent");
        assert!(doc.contains("# Quality Grade Report"));
        assert!(doc.contains("## Domain Coverage"));
    }

    #[test]
    fn render_shows_dimension_details_when_rationale_present() {
        let repo_wide = RepoWideAssessment {
            agent_steering: 71,
            mechanical_guardrails: 85,
            local_feedback_loop: 60,
            coverage_infrastructure: 50,
            documentation_quality: 70,
        };
        let mut rw_rationale = BTreeMap::new();
        rw_rationale.insert(
            "agent_steering".to_string(),
            "AGENTS.md exists but lacks architecture pointers.".to_string(),
        );
        rw_rationale.insert(
            "mechanical_guardrails".to_string(),
            "Linter and CI configured, no pre-commit hooks.".to_string(),
        );
        let payload = AssessmentPayload {
            domains: vec![DomainAssessment {
                name: "core".to_string(),
                languages: vec!["Rust".to_string()],
                scores: DomainScores {
                    test_coverage: 60,
                    test_quality: 50,
                    risk_exposure: 40,
                    convention_adherence: 70,
                },
                notes: vec!["decent".to_string()],
                dimension_rationale: BTreeMap::new(),
            }],
            repo_wide: repo_wide.clone(),
            deficiencies: vec![],
            domain_file_map: Default::default(),
            primary_gap: "steering".to_string(),
            languages_detected: vec!["Rust".to_string()],
            repo_wide_rationale: rw_rationale,
        };
        let report = compute_grade_report(payload);
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        assert!(doc.contains("### Dimension Details"));
        assert!(doc.contains("**Agent Steering** (71 / C+)"));
        assert!(doc.contains("AGENTS.md exists but lacks architecture pointers."));
        assert!(doc.contains("**Mechanical Guardrails** (85 / B)"));
    }

    #[test]
    fn render_shows_domain_score_details_when_rationale_present() {
        let repo_wide = RepoWideAssessment {
            agent_steering: 80,
            mechanical_guardrails: 80,
            local_feedback_loop: 80,
            coverage_infrastructure: 80,
            documentation_quality: 80,
        };
        let mut dim_rationale = BTreeMap::new();
        dim_rationale.insert(
            "test_coverage".to_string(),
            "12 of 15 files have tests.".to_string(),
        );
        dim_rationale.insert(
            "test_quality".to_string(),
            "Assertion density is good at 4.2 per file.".to_string(),
        );
        let payload = AssessmentPayload {
            domains: vec![DomainAssessment {
                name: "api".to_string(),
                languages: vec!["Rust".to_string()],
                scores: DomainScores {
                    test_coverage: 80,
                    test_quality: 70,
                    risk_exposure: 30,
                    convention_adherence: 85,
                },
                notes: vec!["solid".to_string()],
                dimension_rationale: dim_rationale,
            }],
            repo_wide: repo_wide.clone(),
            deficiencies: vec![],
            domain_file_map: Default::default(),
            primary_gap: "none".to_string(),
            languages_detected: vec!["Rust".to_string()],
            repo_wide_rationale: BTreeMap::new(),
        };
        let report = compute_grade_report(payload);
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        assert!(doc.contains("### Domain Score Details"));
        assert!(doc.contains("**api**"));
        assert!(doc.contains("- **Coverage**: 12 of 15 files have tests."));
        assert!(doc.contains("- **Quality**: Assertion density is good at 4.2 per file."));
    }

    #[test]
    fn render_skips_rationale_sections_when_empty() {
        let (report, repo_wide) = sample_report();
        let doc = render_grade_document_with_repo_wide(&report, &repo_wide, "agent");
        // sample_report has empty rationale maps
        assert!(!doc.contains("### Dimension Details"));
        assert!(!doc.contains("### Domain Score Details"));
    }
}
