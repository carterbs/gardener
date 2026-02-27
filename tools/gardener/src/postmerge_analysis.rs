use crate::fsm::MergingOutput;
use crate::logging::append_run_log;
use crate::prompt_knowledge::{score_entry, KnowledgeEntry};
use serde_json::json;

pub fn analyze_postmerge(output: &MergingOutput, evidence: Vec<String>) -> Option<KnowledgeEntry> {
    append_run_log(
        "debug",
        "postmerge.analysis.started",
        json!({
            "merged": output.merged,
            "merge_sha": output.merge_sha,
            "evidence_count": evidence.len()
        }),
    );

    if !output.merged {
        append_run_log(
            "debug",
            "postmerge.analysis.skipped",
            json!({
                "reason": "merge did not succeed"
            }),
        );
        return None;
    }

    let confidence = score_entry(evidence.len());
    let entry = KnowledgeEntry {
        key: "merge_succeeded_with_validation".to_string(),
        confidence,
        evidence,
    };

    append_run_log(
        "info",
        "postmerge.analysis.completed",
        json!({
            "key": entry.key,
            "confidence": confidence,
            "evidence_count": entry.evidence.len(),
            "merge_sha": output.merge_sha
        }),
    );

    Some(entry)
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
