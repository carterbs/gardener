use crate::errors::GardenerError;
use crate::types::WorkerState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptContextItem {
    pub section: String,
    pub source_id: String,
    pub source_hash: String,
    pub rationale: String,
    pub rank: u32,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub section: String,
    pub source_id: String,
    pub source_hash: String,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextManifest {
    pub schema_version: u32,
    pub state: WorkerState,
    pub entries: Vec<ManifestEntry>,
    pub manifest_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPacket {
    pub task_packet: String,
    pub repo_context: String,
    pub evidence_context: String,
    pub execution_context: String,
    pub knowledge_context: String,
    pub context_manifest: ContextManifest,
}

pub fn build_prompt_packet(
    state: WorkerState,
    mut items: Vec<PromptContextItem>,
    token_budget: usize,
) -> Result<PromptPacket, GardenerError> {
    items.sort_by(|a, b| {
        b.rank
            .cmp(&a.rank)
            .then(a.source_hash.cmp(&b.source_hash))
            .then(a.section.cmp(&b.section))
            .then(a.source_id.cmp(&b.source_id))
    });

    let mut selected = Vec::new();
    let mut consumed = 0usize;

    for item in items {
        let tokens = rough_token_count(&item.content);
        if consumed.saturating_add(tokens) > token_budget {
            continue;
        }
        consumed = consumed.saturating_add(tokens);
        selected.push(item);
    }

    let by_section = |name: &str| -> String {
        selected
            .iter()
            .filter(|item| item.section == name)
            .map(|item| item.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };

    let packet = PromptPacket {
        task_packet: by_section("task_packet"),
        repo_context: by_section("repo_context"),
        evidence_context: by_section("evidence_context"),
        execution_context: by_section("execution_context"),
        knowledge_context: by_section("knowledge_context"),
        context_manifest: build_manifest(state, &selected),
    };

    for (name, value) in [
        ("task_packet", &packet.task_packet),
        ("repo_context", &packet.repo_context),
        ("evidence_context", &packet.evidence_context),
        ("execution_context", &packet.execution_context),
        ("knowledge_context", &packet.knowledge_context),
    ] {
        if value.trim().is_empty() {
            return Err(GardenerError::InvalidConfig(format!(
                "missing required packet section: {name}"
            )));
        }
    }

    Ok(packet)
}

fn build_manifest(state: WorkerState, selected: &[PromptContextItem]) -> ContextManifest {
    let mut entries = selected
        .iter()
        .map(|item| ManifestEntry {
            section: item.section.clone(),
            source_id: item.source_id.clone(),
            source_hash: item.source_hash.clone(),
            rationale: item.rationale.clone(),
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| {
        a.section
            .cmp(&b.section)
            .then(a.source_id.cmp(&b.source_id))
            .then(a.source_hash.cmp(&b.source_hash))
    });

    let mut hasher = Sha256::new();
    hasher.update(format!("state={state:?};schema=1\n").as_bytes());
    for entry in &entries {
        hasher.update(
            format!(
                "{}|{}|{}|{}\n",
                entry.section, entry.source_id, entry.source_hash, entry.rationale
            )
            .as_bytes(),
        );
    }

    let manifest_hash = format!("{:x}", hasher.finalize());

    ContextManifest {
        schema_version: 1,
        state,
        entries,
        manifest_hash,
    }
}

fn rough_token_count(text: &str) -> usize {
    text.split_whitespace().count().max(1)
}

#[cfg(test)]
mod tests {
    use super::{build_prompt_packet, PromptContextItem};
    use crate::types::WorkerState;

    fn ctx(
        section: &str,
        source_id: &str,
        source_hash: &str,
        rationale: &str,
        rank: u32,
        content: &str,
    ) -> PromptContextItem {
        PromptContextItem {
            section: section.to_string(),
            source_id: source_id.to_string(),
            source_hash: source_hash.to_string(),
            rationale: rationale.to_string(),
            rank,
            content: content.to_string(),
        }
    }

    #[test]
    fn packet_build_is_deterministic_and_has_required_sections() {
        let items = vec![
            ctx("task_packet", "a", "h3", "task", 100, "task line"),
            ctx("repo_context", "b", "h2", "repo", 90, "repo line"),
            ctx("evidence_context", "c", "h4", "evidence", 80, "evidence line"),
            ctx("execution_context", "d", "h1", "exec", 70, "execution line"),
            ctx("knowledge_context", "e", "h5", "knowledge", 60, "knowledge line"),
        ];

        let p1 = build_prompt_packet(WorkerState::Doing, items.clone(), 1000).expect("packet");
        let p2 = build_prompt_packet(WorkerState::Doing, items, 1000).expect("packet");
        assert_eq!(p1.context_manifest.manifest_hash, p2.context_manifest.manifest_hash);
        assert!(!p1.task_packet.is_empty());
        assert!(!p1.repo_context.is_empty());
        assert!(!p1.evidence_context.is_empty());
        assert!(!p1.execution_context.is_empty());
        assert!(!p1.knowledge_context.is_empty());
    }

    #[test]
    fn token_budget_trimming_can_fail_missing_sections() {
        let items = vec![
            ctx("task_packet", "a", "h1", "task", 1, "task"),
            ctx("repo_context", "b", "h2", "repo", 1, "repo"),
            ctx("evidence_context", "c", "h3", "evidence", 1, "evidence"),
            ctx("execution_context", "d", "h4", "exec", 1, "execution"),
            ctx("knowledge_context", "e", "h5", "knowledge", 1, "knowledge"),
        ];

        let err = build_prompt_packet(WorkerState::Doing, items, 2).expect_err("must fail");
        assert!(format!("{err}").contains("missing required packet section"));
    }
}
