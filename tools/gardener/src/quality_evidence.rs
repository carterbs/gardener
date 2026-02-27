use crate::quality_domain_catalog::QualityDomain;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainEvidence {
    pub domain: String,
    pub tested_files: Vec<String>,
    pub untested_files: Vec<String>,
}

pub fn collect_evidence(domains: &[QualityDomain]) -> Vec<DomainEvidence> {
    domains
        .iter()
        .map(|d| DomainEvidence {
            domain: d.name.clone(),
            tested_files: vec!["src/lib.rs".to_string()],
            untested_files: vec!["src/main.rs".to_string()],
        })
        .collect()
}
