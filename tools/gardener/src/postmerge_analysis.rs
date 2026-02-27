use crate::fsm::MergingOutput;
use crate::prompt_knowledge::{score_entry, KnowledgeEntry};

pub fn analyze_postmerge(output: &MergingOutput, evidence: Vec<String>) -> Option<KnowledgeEntry> {
    if !output.merged {
        return None;
    }

    Some(KnowledgeEntry {
        key: "merge_succeeded_with_validation".to_string(),
        confidence: score_entry(evidence.len()),
        evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::analyze_postmerge;
    use crate::fsm::MergingOutput;

    #[test]
    fn postmerge_analysis_only_emits_for_successful_merges() {
        assert!(analyze_postmerge(
            &MergingOutput {
                merged: false,
                merge_sha: None,
            },
            vec!["x".to_string()]
        )
        .is_none());

        let entry = analyze_postmerge(
            &MergingOutput {
                merged: true,
                merge_sha: Some("abc".to_string()),
            },
            vec!["validated".to_string(), "green".to_string()],
        )
        .expect("entry");
        assert!(entry.confidence > 0.0);
    }
}
