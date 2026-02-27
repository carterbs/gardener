use crate::quality_evidence::DomainEvidence;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainScore {
    pub domain: String,
    pub score: i64,
    pub grade: String,
}

pub fn score_domains(evidence: &[DomainEvidence], has_coverage_gates: bool) -> Vec<DomainScore> {
    evidence
        .iter()
        .map(|item| {
            let base = if has_coverage_gates { 80 } else { 0 };
            let score = base + (item.tested_files.len() as i64 * 5)
                - (item.untested_files.len() as i64 * 2);
            DomainScore {
                domain: item.domain.clone(),
                score,
                grade: if score >= 85 {
                    "A".to_string()
                } else if score >= 70 {
                    "B".to_string()
                } else if score >= 50 {
                    "C".to_string()
                } else if score >= 30 {
                    "D".to_string()
                } else {
                    "F".to_string()
                },
            }
        })
        .collect()
}
