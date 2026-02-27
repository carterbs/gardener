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
            let base = if has_coverage_gates { 80 } else { 0 };
            let score = base + (item.tested_files.len() as i64 * 5)
                - (item.untested_files.len() as i64 * 2);
            let grade = if score >= 85 {
                "A".to_string()
            } else if score >= 70 {
                "B".to_string()
            } else if score >= 50 {
                "C".to_string()
            } else if score >= 30 {
                "D".to_string()
            } else {
                "F".to_string()
            };
            append_run_log(
                "debug",
                "quality.scoring.computed",
                json!({
                    "domain": item.domain,
                    "base": base,
                    "tested_files": item.tested_files.len(),
                    "untested_files": item.untested_files.len(),
                    "score": score,
                    "grade": grade
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
