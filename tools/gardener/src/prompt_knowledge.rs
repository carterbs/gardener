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
        .map(|entry| {
            if entry.evidence.is_empty() {
                return entry.key.clone();
            }
            // Write evidence to a temp file so the agent can read the full CI logs.
            match write_evidence_file(&entry.key, &entry.evidence) {
                Some(path) => format!("evidence_file: {path}"),
                None => entry.key.clone(),
            }
        })
        .collect()
}

/// Write evidence strings to a temp file and return its path.
fn write_evidence_file(key: &str, evidence: &[String]) -> Option<String> {
    use std::io::Write;
    let safe_key: String = key
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = std::env::temp_dir().join(format!("gardener-evidence-{safe_key}.txt"));
    let mut f = std::fs::File::create(&path).ok()?;
    for (i, item) in evidence.iter().enumerate() {
        if i > 0 {
            writeln!(f, "\n---\n").ok()?;
        }
        write!(f, "{item}").ok()?;
    }
    f.flush().ok()?;
    Some(path.to_string_lossy().into_owned())
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
        assert!(lines[0].contains("evidence_file:"));
    }

    #[test]
    fn evidence_file_written_and_referenced() {
        let lines = to_prompt_lines(
            &[KnowledgeEntry {
                key: "ci_fail".to_string(),
                evidence: vec!["error: test failed".to_string(), "line 42".to_string()],
                confidence: 0.9,
            }],
            0.0,
        );
        assert_eq!(lines.len(), 1);
        let path = lines[0]
            .strip_prefix("evidence_file: ")
            .expect("should start with evidence_file:");
        let content = std::fs::read_to_string(path).expect("evidence file should exist");
        assert!(content.contains("error: test failed"));
        assert!(content.contains("line 42"));
        assert!(content.contains("---"));
    }

    #[test]
    fn empty_evidence_skips_file() {
        let lines = to_prompt_lines(
            &[KnowledgeEntry {
                key: "no_ev".to_string(),
                evidence: vec![],
                confidence: 0.5,
            }],
            0.0,
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "no_ev");
        assert!(!lines[0].contains("evidence_file:"));
    }
}
