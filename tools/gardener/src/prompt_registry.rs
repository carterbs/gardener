use crate::errors::GardenerError;
use crate::types::WorkerState;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    pub version: &'static str,
    pub body: &'static str,
}

#[derive(Debug, Clone)]
pub struct PromptRegistry {
    templates: BTreeMap<WorkerState, PromptTemplate>,
}

impl PromptRegistry {
    pub fn v1() -> Self {
        let mut templates = BTreeMap::new();

        templates.insert(WorkerState::Understand, understand_template());
        templates.insert(WorkerState::Planning, planning_template());
        templates.insert(WorkerState::Doing, doing_template());
        templates.insert(WorkerState::Gitting, gitting_template());
        templates.insert(WorkerState::Reviewing, reviewing_template());
        templates.insert(WorkerState::Merging, merging_template());

        Self { templates }
    }

    pub fn template_for(&self, state: WorkerState) -> Result<&PromptTemplate, GardenerError> {
        self.templates.get(&state).ok_or_else(|| {
            GardenerError::InvalidConfig(format!("missing prompt template for state {state:?}"))
        })
    }
}

fn understand_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-understand",
        body: r#"Intent: categorize task as task|chore|infra|feature|bugfix|refactor.
Guardrails: deterministic classification with concise reasoning.
Output schema must be JSON envelope with payload fields: task_type, reasoning.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn planning_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-planning",
        body: r#"Intent: produce a compact execution plan before implementation.
Guardrails: do not edit files in this state; plan only.
Output schema must be JSON envelope with payload fields: summary, milestones.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn doing_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-doing",
        body: r#"Intent: implement changes and verify behavior within current task scope.
Guardrails: max 100 turns, keep patch minimal, include changed files list.
Output schema must be JSON envelope with payload fields: summary, files_changed.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn gitting_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-gitting",
        body: r#"Intent: produce git artifacts (branch/pr metadata) only.
Guardrails: no orchestration logic in runtime; runtime only verifies invariants.
Output schema must be JSON envelope with payload fields: branch, pr_number, pr_url.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn reviewing_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-reviewing",
        body: r#"Intent: review implementation quality and return approve|needs_changes with suggestions.
Guardrails: suggestions must be actionable and scoped.
Output schema must be JSON envelope with payload fields: verdict, suggestions.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn merging_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-merging",
        body: r#"Intent: merge after validation passes; report merge status.
Guardrails: include deterministic merge_sha when merged=true.
Output schema must be JSON envelope with payload fields: merged, merge_sha.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

#[cfg(test)]
mod tests {
    use super::PromptRegistry;
    use crate::types::WorkerState;

    #[test]
    fn registry_contains_v1_worker_templates() {
        let registry = PromptRegistry::v1();
        for state in [
            WorkerState::Understand,
            WorkerState::Planning,
            WorkerState::Doing,
            WorkerState::Gitting,
            WorkerState::Reviewing,
            WorkerState::Merging,
        ] {
            let tpl = registry.template_for(state).expect("template exists");
            assert!(tpl.body.contains("<<GARDENER_JSON_START>>"));
        }
    }
}
