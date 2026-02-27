use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub key: String,
    pub evidence: Vec<String>,
    pub confidence: f64,
}

pub fn score_entry(evidence_count: usize) -> f64 {
    (evidence_count as f64 / 5.0).min(1.0)
}

pub fn decay_confidence(current: f64, decay_per_day: f64, days: f64) -> f64 {
    let retained = (1.0 - decay_per_day).max(0.0);
    (current * retained.powf(days)).max(0.0)
}

pub fn to_prompt_lines(entries: &[KnowledgeEntry], deactivate_below: f64) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| entry.confidence >= deactivate_below)
        .map(|entry| format!("{} (confidence {:.2})", entry.key, entry.confidence))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{decay_confidence, score_entry, to_prompt_lines, KnowledgeEntry};

    #[test]
    fn knowledge_scoring_and_decay_contract() {
        assert_eq!(score_entry(1), 0.2);
        assert_eq!(score_entry(99), 1.0);

        let decayed = decay_confidence(1.0, 0.1, 2.0);
        assert!(decayed < 1.0 && decayed > 0.0);

        let lines = to_prompt_lines(
            &[
                KnowledgeEntry {
                    key: "k1".to_string(),
                    evidence: vec!["a".to_string()],
                    confidence: 0.8,
                },
                KnowledgeEntry {
                    key: "k2".to_string(),
                    evidence: vec!["b".to_string()],
                    confidence: 0.1,
                },
            ],
            0.2,
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("k1"));
    }
}
