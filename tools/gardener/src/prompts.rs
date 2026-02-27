use crate::errors::GardenerError;
use crate::prompt_context::{build_prompt_packet, PromptContextItem, PromptPacket};
use crate::prompt_registry::PromptRegistry;
use crate::types::WorkerState;

#[derive(Debug, Clone)]
pub struct PromptRenderResult {
    pub prompt_version: String,
    pub packet: PromptPacket,
    pub rendered: String,
}

pub fn render_state_prompt(
    registry: &PromptRegistry,
    state: WorkerState,
    items: Vec<PromptContextItem>,
    token_budget: usize,
) -> Result<PromptRenderResult, GardenerError> {
    let template = registry.template_for(state)?;
    let packet = build_prompt_packet(state, items, token_budget)?;
    let rendered = format!(
        "{}\n\n[task_packet]\n{}\n\n[repo_context]\n{}\n\n[evidence_context]\n{}\n\n[execution_context]\n{}\n\n[knowledge_context]\n{}\n\n[context_manifest_hash]\n{}\n",
        template.body,
        packet.task_packet,
        packet.repo_context,
        packet.evidence_context,
        packet.execution_context,
        packet.knowledge_context,
        packet.context_manifest.manifest_hash,
    );

    Ok(PromptRenderResult {
        prompt_version: template.version.to_string(),
        packet,
        rendered,
    })
}

#[cfg(test)]
mod tests {
    use super::render_state_prompt;
    use crate::prompt_context::PromptContextItem;
    use crate::prompt_registry::PromptRegistry;
    use crate::types::WorkerState;

    #[test]
    fn render_includes_prompt_version_and_manifest_hash() {
        let registry = PromptRegistry::v1();
        let result = render_state_prompt(
            &registry,
            WorkerState::Doing,
            vec![
                PromptContextItem {
                    section: "task_packet".to_string(),
                    source_id: "task".to_string(),
                    source_hash: "1".to_string(),
                    rationale: "r".to_string(),
                    rank: 5,
                    content: "task".to_string(),
                },
                PromptContextItem {
                    section: "repo_context".to_string(),
                    source_id: "repo".to_string(),
                    source_hash: "2".to_string(),
                    rationale: "r".to_string(),
                    rank: 4,
                    content: "repo".to_string(),
                },
                PromptContextItem {
                    section: "evidence_context".to_string(),
                    source_id: "evidence".to_string(),
                    source_hash: "3".to_string(),
                    rationale: "r".to_string(),
                    rank: 3,
                    content: "evidence".to_string(),
                },
                PromptContextItem {
                    section: "execution_context".to_string(),
                    source_id: "execution".to_string(),
                    source_hash: "4".to_string(),
                    rationale: "r".to_string(),
                    rank: 2,
                    content: "execution".to_string(),
                },
                PromptContextItem {
                    section: "knowledge_context".to_string(),
                    source_id: "knowledge".to_string(),
                    source_hash: "5".to_string(),
                    rationale: "r".to_string(),
                    rank: 1,
                    content: "knowledge".to_string(),
                },
            ],
            100,
        )
        .expect("rendered");

        assert!(result.prompt_version.starts_with("v1-"));
        assert!(result.rendered.contains("[context_manifest_hash]"));
        assert!(result
            .rendered
            .contains(&result.packet.context_manifest.manifest_hash));
    }
}
