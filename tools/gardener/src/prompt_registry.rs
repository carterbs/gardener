use crate::config::GitOutputMode;
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
        templates.insert(WorkerState::Gitting, gitting_template_pr());
        templates.insert(WorkerState::Reviewing, reviewing_template());
        templates.insert(WorkerState::Merging, merging_template_local());

        Self { templates }
    }

    pub fn with_gitting_mode(mut self, mode: &GitOutputMode) -> Self {
        let template = match mode {
            GitOutputMode::CommitOnly => gitting_template_commit_only(),
            GitOutputMode::Push => gitting_template_push(),
            GitOutputMode::PullRequest => gitting_template_pr(),
        };
        self.templates.insert(WorkerState::Gitting, template);
        self
    }

    pub fn with_merging_mode(mut self, mode: &GitOutputMode) -> Self {
        let template = match mode {
            GitOutputMode::PullRequest => merging_template_pr(),
            GitOutputMode::CommitOnly | GitOutputMode::Push => merging_template_local(),
        };
        self.templates.insert(WorkerState::Merging, template);
        self
    }

    pub fn with_retry_rebase(mut self, attempt_count: i64) -> Self {
        if attempt_count > 1 {
            self.templates
                .insert(WorkerState::Doing, doing_template_retry_rebase());
        }
        self
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
Output schema must be JSON envelope with payload fields: summary, files_changed, commit_message.
commit_message must be a concise conventional-commit style message describing what was implemented.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn doing_template_retry_rebase() -> PromptTemplate {
    PromptTemplate {
        version: "v1-doing-retry-rebase",
        body: r#"Intent: rebase onto latest main, resolve any conflicts, then implement changes and verify behavior within current task scope.

Step 1 — Rebase onto main:
  Run: git fetch origin main && git rebase origin/main
  If conflicts occur: resolve them using your knowledge of the task context and existing commits, then git add the resolved files and git rebase --continue. Repeat until the rebase completes.
  If the rebase succeeds cleanly, proceed to step 2.

Step 2 — Implement:
  Implement changes and verify behavior within current task scope.
  Guardrails: max 100 turns, keep patch minimal, include changed files list.

Output schema must be JSON envelope with payload fields: summary, files_changed, commit_message.
commit_message must be a concise conventional-commit style message describing what was implemented.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn gitting_template_commit_only() -> PromptTemplate {
    PromptTemplate {
        version: "v1-gitting-commit-only",
        body: r#"Intent: stage and commit all changes on the current branch.
Run: git add -A followed by git commit with a clear, conventional-commit style message describing what was implemented.
Guardrails: do not push to remote; do not modify source files.
Output schema must be JSON envelope with payload fields: branch, pr_number, pr_url.
pr_number must be 0 and pr_url must be an empty string.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn gitting_template_push() -> PromptTemplate {
    PromptTemplate {
        version: "v1-gitting-push",
        body: r#"Intent: stage and commit all changes, then push the branch to origin.
Run: git add -A, then git commit with a clear conventional-commit style message, then git push origin <branch>.
Guardrails: do not open a pull request; do not modify source files.
Output schema must be JSON envelope with payload fields: branch, pr_number, pr_url.
pr_number must be 0 and pr_url must be an empty string.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn gitting_template_pr() -> PromptTemplate {
    PromptTemplate {
        version: "v1-gitting-pr",
        body: r#"Intent: stage and commit all changes, push the branch, then open a GitHub pull request.
Run: git add -A, then git commit with a clear conventional-commit style message, then git push origin <branch>, then gh pr create.
The PR title and body must be written thoughtfully: summarize what was built and why, call out any non-obvious decisions, and make it easy for a reviewer to understand the scope of the change. Do not use the task ID as the title. Write like a human engineer who cares about the reviewer's time.
Guardrails: do not modify source files; only git and gh operations are permitted.
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

fn merging_template_local() -> PromptTemplate {
    PromptTemplate {
        version: "v1-merging-local",
        body: r#"Intent: merge the current worktree branch into main on the local repo and report the resulting merge commit SHA.
Run: from the repo root (not the worktree), run git merge --no-ff <current-branch> and capture the resulting commit SHA with git rev-parse HEAD.
Guardrails: do not push; do not open a pull request; do not modify source files; include the deterministic merge_sha when merged=true.
Output schema must be JSON envelope with payload fields: merged, merge_sha.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn merging_template_pr() -> PromptTemplate {
    PromptTemplate {
        version: "v1-merging-pr",
        body: r#"Intent: merge the open GitHub pull request for the current branch and report the resulting merge commit SHA.
Run: use gh pr merge --merge --auto or gh pr merge <pr-number> --merge to merge the PR, then capture the merge commit SHA.
Guardrails: do not perform a local git merge; do not modify source files; include the deterministic merge_sha when merged=true.
Output schema must be JSON envelope with payload fields: merged, merge_sha.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

#[cfg(test)]
mod tests {
    use super::PromptRegistry;
    use crate::types::WorkerState;

    #[test]
    fn with_retry_rebase_swaps_doing_template_on_retry() {
        let registry = PromptRegistry::v1().with_retry_rebase(2);
        let tpl = registry
            .template_for(WorkerState::Doing)
            .expect("template exists");
        assert_eq!(tpl.version, "v1-doing-retry-rebase");
        assert!(tpl.body.contains("git fetch origin main && git rebase origin/main"));
    }

    #[test]
    fn with_retry_rebase_noop_on_first_attempt() {
        let registry = PromptRegistry::v1().with_retry_rebase(1);
        let tpl = registry
            .template_for(WorkerState::Doing)
            .expect("template exists");
        assert_eq!(tpl.version, "v1-doing");
    }

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
