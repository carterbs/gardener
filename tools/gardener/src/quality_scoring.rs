use crate::logging::append_run_log;
use crate::quality_evidence::DomainEvidence;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainScore {
    pub domain: String,
    pub score: i64,
    pub grade: String,
}

pub fn score_domains(evidence: &[DomainEvidence], has_coverage_gates: bool) -> Vec<DomainScore> {
    append_run_log(
        "debug",
        "quality.scoring.started",
        json!({
            "domain_count": evidence.len(),
            "has_coverage_gates": has_coverage_gates
        }),
    );

    let scores: Vec<DomainScore> = evidence
        .iter()
        .map(|item| {
            let source_count = item.source_files.len() as f64;
            let tested_count = (item.inline_test_files.len() + item.integration_tests.len()) as f64;
            let tested_ratio = if source_count > 0.0 {
                tested_count / source_count
            } else {
                0.0
            };

            let instrumentation_ratio = if source_count > 0.0 {
                item.instrumentation_files.len() as f64 / source_count
            } else {
                0.0
            };

            let mut score = if has_coverage_gates { 30.0 } else { 10.0 };
            score += tested_ratio * 45.0;
            score += instrumentation_ratio * 10.0;
            if !item.integration_tests.is_empty() {
                score += 10.0;
            }
            if item
                .inline_test_files
                .iter()
                .any(|path| path.contains("/quality"))
            {
                score += 5.0;
            }

            if source_count == 0.0 {
                score = 60.0;
            }

            let score = score.clamp(0.0, 100.0).round() as i64;
            let grade = if score >= 90 {
                "A".to_string()
            } else if score >= 75 {
                "B".to_string()
            } else if score >= 55 {
                "C".to_string()
            } else if score >= 35 {
                "D".to_string()
            } else {
                "F".to_string()
            };

            append_run_log(
                "debug",
                "quality.scoring.computed",
                json!({
                    "domain": item.domain,
                    "source_count": item.source_files.len(),
                    "inline_test_files": item.inline_test_files.len(),
                    "integration_tests": item.integration_tests.len(),
                    "instrumentation_files": item.instrumentation_files.len(),
                    "score": score,
                    "grade": grade,
                }),
            );

            DomainScore {
                domain: item.domain.clone(),
                score,
                grade,
            }
        })
        .collect();

    let grades: Vec<&str> = scores.iter().map(|s| s.grade.as_str()).collect();
    append_run_log(
        "info",
        "quality.scoring.completed",
        json!({
            "domain_count": scores.len(),
            "grades": grades
        }),
    );

    scores
}
