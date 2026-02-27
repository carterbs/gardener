use crate::logging::append_run_log;
use crate::postmerge_analysis::analyze_postmerge;
use crate::postmortem::analyze_failure;
use crate::prompt_knowledge::KnowledgeEntry;
use crate::{fsm::MergingOutput, types::WorkerState};
use serde_json::json;

#[derive(Debug, Clone, Default)]
pub struct LearningLoop {
    entries: Vec<KnowledgeEntry>,
}

impl LearningLoop {
    pub fn ingest_postmerge(&mut self, output: &MergingOutput, evidence: Vec<String>) {
        append_run_log(
            "debug",
            "learning_loop.ingest_postmerge.started",
            json!({
                "merged": output.merged,
                "merge_sha": output.merge_sha,
                "evidence_count": evidence.len()
            }),
        );

        if let Some(entry) = analyze_postmerge(output, evidence) {
            append_run_log(
                "info",
                "learning_loop.entry.added",
                json!({
                    "source": "postmerge",
                    "key": entry.key,
                    "confidence": entry.confidence,
                    "total_entries": self.entries.len() + 1
                }),
            );
            self.entries.push(entry);
        } else {
            append_run_log(
                "debug",
                "learning_loop.ingest_postmerge.no_entry",
                json!({
                    "reason": "merge did not succeed"
                }),
            );
        }
    }

    pub fn ingest_failure(&mut self, state: WorkerState, reason: &str, evidence: Vec<String>) {
        let compound_reason = format!("{:?}:{}", state, reason);
        append_run_log(
            "info",
            "learning_loop.ingest_failure.started",
            json!({
                "state": state.as_str(),
                "reason": reason,
                "evidence_count": evidence.len()
            }),
        );

        let entry = analyze_failure(&compound_reason, evidence);
        append_run_log(
            "info",
            "learning_loop.entry.added",
            json!({
                "source": "failure",
                "key": entry.key,
                "confidence": entry.confidence,
                "total_entries": self.entries.len() + 1
            }),
        );
        self.entries.push(entry);
    }

    pub fn entries(&self) -> &[KnowledgeEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::LearningLoop;
    use crate::{fsm::MergingOutput, types::WorkerState};

    #[test]
    fn learning_loop_collects_postmerge_and_failure_entries() {
        let mut loop_state = LearningLoop::default();

        loop_state.ingest_postmerge(
            &MergingOutput {
                merged: true,
                merge_sha: Some("abc".to_string()),
            },
            vec!["e1".to_string()],
        );
        loop_state.ingest_failure(
            WorkerState::Reviewing,
            "suggestions exhausted",
            vec!["e2".to_string()],
        );

        assert_eq!(loop_state.entries().len(), 2);
    }
}
