use crate::postmerge_analysis::analyze_postmerge;
use crate::postmortem::analyze_failure;
use crate::prompt_knowledge::KnowledgeEntry;
use crate::{fsm::MergingOutput, types::WorkerState};

#[derive(Debug, Clone, Default)]
pub struct LearningLoop {
    entries: Vec<KnowledgeEntry>,
}

impl LearningLoop {
    pub fn ingest_postmerge(&mut self, output: &MergingOutput, evidence: Vec<String>) {
        if let Some(entry) = analyze_postmerge(output, evidence) {
            self.entries.push(entry);
        }
    }

    pub fn ingest_failure(&mut self, state: WorkerState, reason: &str, evidence: Vec<String>) {
        self.entries.push(analyze_failure(
            &format!("{:?}:{}", state, reason),
            evidence,
        ));
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
