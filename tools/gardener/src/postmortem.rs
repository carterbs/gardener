use crate::logging::append_run_log;
use crate::prompt_knowledge::{score_entry, KnowledgeEntry};
use serde_json::json;

pub fn analyze_failure(reason: &str, evidence: Vec<String>) -> KnowledgeEntry {
    let key = format!("failure:{}", reason.trim().replace(' ', "_"));
    let confidence = score_entry(evidence.len());

    append_run_log(
        "info",
        "postmortem.generated",
        json!({
            "key": key,
            "confidence": confidence,
            "evidence_count": evidence.len(),
            "reason": reason
        }),
    );

    KnowledgeEntry {
        key,
        confidence,
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
