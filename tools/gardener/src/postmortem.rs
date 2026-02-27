use crate::prompt_knowledge::{score_entry, KnowledgeEntry};

pub fn analyze_failure(reason: &str, evidence: Vec<String>) -> KnowledgeEntry {
    KnowledgeEntry {
        key: format!("failure:{}", reason.trim().replace(' ', "_")),
        confidence: score_entry(evidence.len()),
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::analyze_failure;

    #[test]
    fn postmortem_encodes_reason_into_knowledge_key() {
        let entry = analyze_failure("review loop cap", vec!["log-1".to_string()]);
        assert!(entry.key.contains("review_loop_cap"));
    }
}
