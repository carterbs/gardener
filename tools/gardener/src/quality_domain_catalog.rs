#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityDomain {
    pub name: String,
}

pub fn discover_domains() -> Vec<QualityDomain> {
    vec![QualityDomain {
        name: "core".to_string(),
    }]
}
