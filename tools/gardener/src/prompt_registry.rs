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
        templates.insert(WorkerState::Reviewing, reviewing_template());
        templates.insert(WorkerState::Merging, merge_remediation_template());

        Self { templates }
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
        body: r#"Intent: categorize the incoming task into exactly one of: task|chore|infra|feature|bugfix|refactor.

## Classification guide

- **feature**: new user-facing functionality that did not exist before.
- **bugfix**: corrects incorrect behavior — something that worked before and broke, or never worked as specified.
- **refactor**: restructures existing code without changing external behavior. Includes renames, extraction, and architecture changes.
- **chore**: routine maintenance — dependency updates, config tweaks, CI changes, doc fixes.
- **infra**: tooling, test infrastructure, linters, build system, dev-loop scaffolding, or observability that supports development but is not user-facing.
- **task**: catch-all for work that does not fit the above categories.

## Steps

1. Read the task description from [task_packet] carefully.
2. Classify based on the primary intent of the work, not secondary side effects.
3. Write concise reasoning (1-3 sentences) explaining your classification.

Guardrails: deterministic classification with concise reasoning. Do not modify any files.
Output schema must be JSON envelope with payload fields: task_type, reasoning.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn planning_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-planning",
        body: r#"Intent: produce a detailed execution plan before implementation.

Your job is ONLY to plan — do NOT edit source files, create files, or implement anything.

## Steps

1. Read the task description from [task_packet] thoroughly.
2. Read relevant source files and project conventions to understand the area this task touches.
3. Identify every file that will need to be created or modified, with specifics about what changes go where.
4. Design a test strategy: what tests to write and what they verify.
5. Note any project conventions that apply (naming, file structure, architecture constraints).

## Plan quality

The plan must be detailed enough that the implementation step can execute it without needing to re-research the codebase. Include:
- **summary**: a one-line conventional-commit style title (e.g. "feat: add backlog pruning command", "fix: correct state transition on timeout"). Use one of: feat, fix, chore, refactor, test, docs, ci, perf.
- **milestones**: an ordered list of concrete implementation steps. Each milestone should name the files involved, describe what to build, and call out any non-obvious decisions. Keep milestones small and verifiable — a reviewer should be able to check each one independently.

Do not hand-wave. "Update the handler" is not a milestone. "Add a `prune` match arm to `BacklogCommand::execute` in `src/backlog/commands.rs` that removes entries older than the configured retention window" is.

Guardrails: do not edit files in this state; plan only.
Output schema must be JSON envelope with payload fields: summary, milestones.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn doing_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-doing",
        body: r#"Intent: implement changes and verify behavior within current task scope.

## Steps

1. Read the task description from [task_packet] and the plan context from [knowledge_context].
2. Read relevant project conventions and existing source files before writing any code.
3. Implement changes following the plan. Keep the patch minimal — only touch files that are necessary to complete the task.
4. Write tests for new functionality. Tests should be meaningful, not just existence checks.
5. Run the project's test and lint commands to verify your changes pass.
6. If tests or lints fail, fix the issues before returning.

## Implementation quality

- Follow existing patterns in the codebase. Read neighboring code to match style, naming, and structure.
- Do not refactor surrounding code unless the task explicitly calls for it.
- Do not add speculative features, extra configuration, or "nice to have" improvements beyond scope.
- Keep changes focused. Three similar lines of code are better than a premature abstraction.

## Verification (mandatory)

After implementation, you MUST verify your work actually works:
- Run tests and confirm they pass.
- If you built a new command or handler, exercise it and verify the output.
- If you modified existing behavior, confirm the change is observable.
- Do not just trust that your code is correct — run it and check.

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

## Step 1 — Rebase onto main

Run: git fetch origin main && git rebase origin/main
If conflicts occur: resolve them using your knowledge of the task context and existing commits, then git add the resolved files and git rebase --continue. Repeat until the rebase completes.
Keep behavior from both sides where appropriate — do not silently drop changes from either branch.
If the rebase succeeds cleanly, proceed to step 2.

## Step 2 — Implement

1. Read the task description from [task_packet] and the plan context from [knowledge_context].
2. Read relevant project conventions and existing source files before writing any code.
3. Implement changes following the plan. Keep the patch minimal — only touch files that are necessary.
4. Write tests for new functionality. Tests should be meaningful, not just existence checks.
5. Run the project's test and lint commands to verify your changes pass.
6. If tests or lints fail, fix the issues before returning.

Follow existing patterns in the codebase. Do not refactor surrounding code unless the task calls for it. Do not add speculative features beyond scope.

## Verification (mandatory)

After implementation, verify your work actually works:
- Run tests and confirm they pass.
- If you built a new command or handler, exercise it and verify the output.
- Do not just trust that your code is correct — run it and check.

Guardrails: max 100 turns, keep patch minimal, include changed files list.
Output schema must be JSON envelope with payload fields: summary, files_changed, commit_message.
commit_message must be a concise conventional-commit style message describing what was implemented.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn reviewing_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-reviewing",
        body: r#"Intent: review implementation quality and return approve|needs_changes with suggestions.

You are an independent reviewer. Your job is to ensure the implementation is correct, well-tested, and follows project conventions.

## Steps

1. Read the task description from [task_packet] to understand what was requested.
2. Read the plan and prior context from [knowledge_context] to understand what was intended.
3. Examine the diff — read every changed file and understand the full scope of modifications.
4. Run the project's test and lint commands to verify the implementation passes.
5. Evaluate the implementation against the criteria below.

## Evaluation criteria

- **Correctness**: Does the code do what the task requested? Are there edge cases that are mishandled or silently ignored?
- **Tests**: Are new code paths tested? Are the tests meaningful — do they verify behavior, not just that code runs without crashing? Is coverage adequate for the scope of the change?
- **Conventions**: Does the code follow project naming, file structure, and architecture conventions?
- **Scope**: Are the changes focused on the task, or does the implementation include unrelated refactors, speculative features, or unnecessary abstractions?
- **Quality**: Is the code clear and maintainable? Are there obvious simplifications? Would a human reviewer flag anything as over-engineered or under-documented?

## Verdict

- If the implementation meets all criteria: verdict = "approve", suggestions = [].
- If there are issues: verdict = "needs_changes", suggestions = a list of specific, actionable findings. Each suggestion should name the file and describe what needs to change and why. Do not give vague feedback like "improve tests" — say exactly which cases are missing.

Guardrails: do not modify any files. Suggestions must be actionable and scoped to the current change.
Output schema must be JSON envelope with payload fields: verdict, suggestions.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn merge_remediation_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-merge-remediation",
        body: r#"Intent: fix this PR so it can be merged. The automated merge attempt failed.

## Context

The deterministic merge pipeline tried to merge your PR and failed. Information about the failure is provided in [knowledge_context].

## Possible fixes

- If there are merge conflicts: resolve the conflicting files so the code is correct.
- If CI is failing: identify and fix the test/lint/build failures.
- If the branch is behind main: rebase onto origin/main and resolve any resulting conflicts.

## Rules

- Do NOT run git push, git commit, gh pr merge, or any other git/gh commands that move code.
- Just fix the source files. Your changes will be committed and pushed automatically by the pipeline.
- Run the project's validation command to verify your fixes before returning.

Guardrails: do not run git/gh commands; only fix source files.
Output schema must be JSON envelope with payload fields: summary, files_changed.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

#[allow(dead_code)] // wired up when post-merge validation creates a fix PR
fn post_merge_fix_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-post-merge-fix",
        body: r#"Intent: fix a post-merge validation failure on main.

The PR was merged successfully but validation on the combined main state fails.
The validation error is provided in [knowledge_context].

## Steps

1. Investigate the validation failure output to understand what broke.
2. Identify whether the failure is from your merged changes interacting with other recent changes on main.
3. Fix the code so validation passes.
4. Run the project's validation command to confirm the fix.

## Rules

- Do NOT run git push, git commit, or any other git/gh commands that move code.
- Just fix the source files. Your changes will be committed and pushed automatically.

Guardrails: do not run git/gh commands; only fix source files.
Output schema must be JSON envelope with payload fields: summary, files_changed.
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
        assert!(tpl
            .body
            .contains("git fetch origin main && git rebase origin/main"));
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
            WorkerState::Reviewing,
            WorkerState::Merging,
        ] {
            let tpl = registry.template_for(state).expect("template exists");
            assert!(tpl.body.contains("<<GARDENER_JSON_START>>"));
        }
    }

    #[test]
    fn merge_remediation_template_prohibits_git_commands() {
        let registry = PromptRegistry::v1();
        let tpl = registry
            .template_for(WorkerState::Merging)
            .expect("template exists");
        assert_eq!(tpl.version, "v1-merge-remediation");
        assert!(tpl.body.contains("do not run git/gh commands"));
    }
}
