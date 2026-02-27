#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrAuditSummary {
    pub collisions_found: usize,
    pub collisions_fixed: usize,
}

pub fn reconcile_open_prs() -> PrAuditSummary {
    PrAuditSummary::default()
}
