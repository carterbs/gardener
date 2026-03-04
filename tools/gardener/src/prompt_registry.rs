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

pub const SEEDING_PROMPT_VERSION_SEEDING: &str = "seeding-v5";

pub const SEEDING_ACTION_CONTRACT_DRY_RUN: &str = r#"Output contract
- Emit JSON only.
- Return either:
  - {"tasks":[...]}
  - {"schema_version":1,"state":"seeding","payload":{"tasks":[...]}}
- Each task must include: title, details, rationale, domain, priority.
- priority must be one of P0, P1, P2.
- details and rationale must be concrete and actionable in this repository.
- details must include at least one concrete evidence anchor (file path, docs section, quality-grade row, or existing backlog reference)."#;

pub const SEEDING_ACTION_CONTRACT_WRITE: &str = r#"Action contract
- Use the backlog-db skill to insert each task directly into the backlog.
- For each task run:
  ./scripts/backlog-db.sh add --title "..." --details "..." --priority P0|P1|P2 --scope <domain> --kind maintenance
- Do NOT emit JSON. Do NOT print a task list. Insert using the script only.
- priority must be one of P0, P1, P2.
- details must be concrete and actionable in this repository.
- details must include at least one concrete evidence anchor (file path, docs section, quality-grade row, or existing backlog reference)."#;

pub fn seeding_prompt_template() -> PromptTemplate {
    PromptTemplate {
        version: SEEDING_PROMPT_VERSION_SEEDING,
        body: r#"You are the Gardener backlog seeding worker.
Goal: identify 10 actionable tasks that improve repository hospitality for agents.
Do not implement code changes. Do not propose product behavior changes.
Generate exactly 10 tasks that improve repository maintainability, clarity, reliability, or safety for future agents.

{ACTION_CONTRACT}

System framing
- Do not invent nonexistent files, architecture, or conventions.
- Use AGENTS.md, CLAUDE.md, docs listing, and quality grades as source of truth.
- Before beginning, read docs/references/codex-agent-team-article.md.
- Every task must be something a runtime worker can start immediately with the repository context provided.
- Favor tasks that reduce agent friction, including onboarding, diagnostics, workflow, guardrails, and evidence gathering.
- Prefer work types like: docs, testing, tooling, observability, cleanup, onboarding.

Inputs
- primary_gap: {PRIMARY_GAP}
- readiness_score: {READINESS_SCORE}
- readiness_grade: {READINESS_GRADE}

Quality risks extracted from report
{QUALITY_RISKS}

Structural deficiencies identified by quality grading
{STRUCTURAL_DEFICIENCIES}

Domain-level assessment notes
{DOMAIN_NOTES}

Relevant repo anchors
1) AGENTS.md
{AGENTS_MD}
2) CLAUDE.md
{CLAUDE_MD}
3) docs/
{DOCS_LISTING}
4) docs/references/codex-agent-team-article.md
Read this file directly before writing tasks.

Existing active backlog snapshot
{EXISTING_BACKLOG}
{REJECTED_TASKS}
Backlog DB skill reference (.codex/skills/backlog-db/SKILL.md)
{BACKLOG_SKILL_MD}

Task contract
Create exactly 10 tasks. Prefer a practical mix of immediate fixes and cleanup debt.
- At least 2 tasks should map to primary_gap.
- At least 2 tasks should address structural deficiencies identified by quality grading (if any exist).
- At least 2 tasks should be cleanup/debt reduction tasks.
- At least 4 tasks should directly reduce repository friction for agents (docs, automation, diagnostics, workflow).
- priority must be one of P0, P1, P2.
- domain should be concrete and align to discovered file families.
- rationale must explain why this task makes coding agents more effective in this repository.
- details must include at least one concrete evidence anchor (file path, docs section, quality-grade row, or existing backlog reference).

Prioritization policy (effort vs impact)
- Order tasks by effort-to-impact ratio: low effort + high impact first.
- Prefer quick quality-grade lifts before broad tooling buildouts when impact is similar.
- Favor missing tests for existing high-risk code paths over speculative framework additions.
- Avoid filler tasks to reach count; every task must clear a clear impact threshold for this repository.

Seed-generation contract
1. Read docs/quality-grades.md, docs/quality-grades/*.md (if present), AGENTS.md, docs/conventions/, and docs/references/codex-agent-team-article.md.
2. Inspect docs/ and repository structure for concrete, non-duplicate work that helps agents move faster safely.
3. Generate exactly 10 tasks.
4. Ensure at least 2 tasks map to primary_gap.
5. Ensure at least 2 tasks are explicit cleanup/debt reduction tasks.
6. Ensure at least 4 tasks explicitly reduce agent friction (onboarding, evidence, diagnostics, workflow, guardrails).
7. Rank tasks by effort-to-impact ratio (quick wins first).
8. Ensure each task details field includes a concrete evidence anchor (path/section/grade/backlog reference).
9. Ensure there are no product feature tasks.
10. Deliver per the action contract defined above.

Quality doc
{QUALITY_DOC}
"#,
    }
}

impl PromptRegistry {
    pub fn v1() -> Self {
        let mut templates = BTreeMap::new();

        templates.insert(WorkerState::Understand, understand_template());
        templates.insert(WorkerState::Planning, planning_template());
        templates.insert(WorkerState::Doing, doing_template());
        templates.insert(WorkerState::Gitting, gitting_remediation_template());
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
The task has already been selected. Do NOT re-evaluate scope or pick alternative work.

## Steps

1. Read the task description from [task_packet] thoroughly.
2. Read relevant source files and project conventions to understand the area this task touches.
3. Identify every file that will need to be created or modified, with specifics about what changes go where.
4. Design a test strategy: what tests to write and what they verify.
5. Note any project conventions that apply (naming, file structure, architecture constraints).

## Plan quality

The plan must be detailed enough that the implementation step can execute it without needing to re-research the codebase. Include:
- **summary**: a one-line conventional-commit style title (e.g. "feat: add backlog pruning command", "fix: correct state transition on timeout"). Use one of: feat, fix, chore, refactor, test, docs, ci, perf.
- **milestones**: an ordered list of concrete implementation steps. Each milestone must include:
  - what to build (specific behavior, not generic verbs)
  - exact files to create/modify
  - tests to add/update and what they verify
  - QA checks to run beyond "tests passed"
  - relevant conventions/constraints that apply

Do not hand-wave. "Update the handler" is not a milestone. "Add a `prune` match arm to `BacklogCommand::execute` in `src/backlog/commands.rs` that removes entries older than the configured retention window" is.
Do not use placeholders like "update code", "fix stuff", or "improve tests". Be concrete.

Guardrails: do not edit files in this state; plan only.
Output schema must be JSON envelope with payload fields: summary, milestones.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn doing_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-doing",
        body: r#"Intent: implement changes and verify behavior within current task scope.

Execution contract: [knowledge_context] is the implementation plan context.
Execute that plan faithfully. Do NOT re-plan, re-scope, or choose alternate tasks unless a concrete contradiction blocks implementation.

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
- Do not edit unrelated shared coordination/state files unless the task explicitly requires it.
- Do not bypass quality gates. If tests/lints/coverage checks fail, fix code/tests instead of weakening checks.
- Do not lower thresholds or expand excludes/ignore lists (coverage, lint, test, validation config) unless the task explicitly requires a policy change.

## Verification (mandatory)

After implementation, you MUST verify your work actually works:
- Run tests and confirm they pass.
- If you built a new command or handler, exercise it and verify the output.
- If you modified existing behavior, confirm the change is observable.
- If you built tooling/linting/automation, run it and verify it catches or produces the expected result.
- Do not stop at static validation when runtime behavior can be exercised; run the thing end-to-end in scope.
- Do not just trust that your code is correct — run it and check.

Guardrails: max 100 turns, keep patch minimal.
After all verification passes, commit your work: `git add -A && git commit -m "<msg>"` where <msg> is a conventional-commit subject describing what you implemented (e.g. "feat: enable clippy::needless_update", "fix: correct state transition on timeout"). Do not use generic messages like "implement task changes".
Output schema must be JSON envelope with payload fields: summary.
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

Execution contract: [knowledge_context] is the implementation plan context.
Execute that plan faithfully. Do NOT re-plan, re-scope, or choose alternate tasks unless a concrete contradiction blocks implementation.

1. Read the task description from [task_packet] and the plan context from [knowledge_context].
2. Read relevant project conventions and existing source files before writing any code.
3. Implement changes following the plan. Keep the patch minimal — only touch files that are necessary.
4. Write tests for new functionality. Tests should be meaningful, not just existence checks.
5. Run the project's test and lint commands to verify your changes pass.
6. If tests or lints fail, fix the issues before returning.

Follow existing patterns in the codebase. Do not refactor surrounding code unless the task calls for it. Do not add speculative features beyond scope.
- Do not bypass quality gates. If tests/lints/coverage checks fail, fix code/tests instead of weakening checks.
- Do not lower thresholds or expand excludes/ignore lists (coverage, lint, test, validation config) unless the task explicitly requires a policy change.

## Verification (mandatory)

After implementation, verify your work actually works:
- Run tests and confirm they pass.
- If you built a new command or handler, exercise it and verify the output.
- If you built tooling/linting/automation, run it and verify it catches or produces the expected result.
- Do not stop at static validation when runtime behavior can be exercised; run the thing end-to-end in scope.
- Do not just trust that your code is correct — run it and check.

Guardrails: max 100 turns, keep patch minimal.
After all verification passes, commit your work: `git add -A && git commit -m "<msg>"` where <msg> is a conventional-commit subject describing what you implemented (e.g. "feat: enable clippy::needless_update", "fix: correct state transition on timeout"). Do not use generic messages like "implement task changes".
Output schema must be JSON envelope with payload fields: summary.
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
- **Integrity of checks**: Did implementation preserve validation standards? Any attempt to weaken gates (for example coverage ignores/excludes or lower thresholds) must be treated as a failure unless explicitly requested by the task.

## Verdict

- If the implementation meets all criteria: verdict = "approve", suggestions = [].
- If there are issues: verdict = "needs_changes", suggestions = a list of specific, actionable findings. Each suggestion should name the file and describe what needs to change and why. Do not give vague feedback like "improve tests" — say exactly which cases are missing.
- Fail closed: if required evidence is missing, validation was not run, or output is ambiguous, verdict must be "needs_changes" with specific missing evidence/actions.

Guardrails: do not modify any files. Suggestions must be actionable and scoped to the current change.
Output schema must be JSON envelope with payload fields: verdict, suggestions.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

fn gitting_remediation_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-gitting-remediation",
        body: r#"Intent: remediate git publication failures after deterministic push handling failed.

The deterministic gitting pipeline failed to publish this branch. Read the evidence file in [knowledge_context] for failure details.
This is remediation-only work. Keep changes minimal and strictly tied to the reported failure.

## Steps

1. Read the evidence file referenced in [knowledge_context] and identify the exact cause of the publication failure.
2. Inspect and fix source files as needed (for example unresolved conflict markers, missing merged changes, or failing validation-related edits).
3. Run the project validation command to confirm the worktree is publish-ready.
4. Return a concise summary and changed files.

## Rules

- Do NOT run git push, git commit, gh pr create, gh pr merge, or any other git/gh commands that move code.
- You may inspect git state (`git status`, `git diff`, `git show`) only for diagnosis.
- Keep changes scoped to making publication deterministic and safe.
- Do NOT "fix" failures by weakening checks (for example changing coverage/lint/test ignore lists or lowering thresholds) unless explicitly requested.

Guardrails: do not move git state; only fix source files and validate.
When your turn is complete, stop after two plain-text lines:
DONE: <what you changed>
VALIDATION: <command(s) run and pass/fail outcome>"#,
    }
}

pub fn pr_creation_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-pr-creation",
        body: r#"Intent: create a pull request for the changes on this branch.

## Steps

1. Run `git log main..HEAD --oneline` to see what commits are on this branch.
2. Run `git diff main..HEAD` to understand what changed.
3. Run `gh pr create --title "<title>" --body "<body>"` to open the pull request.
   - Title must use conventional commit format: `<type>: <description>` (max 72 chars).
     Valid types: feat, fix, refactor, test, chore, docs, perf, style.
     Base the title on the diff content, not a generic restatement of the task description.
   - Body: 2-4 sentences or a brief bulleted list describing what changed and why. Be specific.
   - If `gh pr create` reports the PR already exists, that is fine — stop.

## Rules

- Do NOT commit, push, or modify any source files.
- Run exactly one `gh pr create` call."#,
    }
}

fn merge_remediation_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-merge-remediation",
        body: r#"Intent: fix this PR so it can be merged. The automated merge attempt failed.

## Context

The deterministic merge pipeline tried to merge your PR and failed. Read the evidence file in [knowledge_context] for failure details.

## Possible fixes

- If there are merge conflicts: resolve the conflicting files so the code is correct.
- If CI is failing: identify and fix the test/lint/build failures.
- If the branch is behind main: rebase onto origin/main and resolve any resulting conflicts.

## Autonomy

You have full autonomy. Fix the code, commit your changes, and push. The pipeline is the safety net.
- Run the project's validation command to verify your fixes before pushing.
- Keep edits minimal and mergeability-focused; avoid unrelated refactors.
- Do NOT "fix" failures by weakening checks (for example changing coverage/lint/test ignore lists or lowering thresholds) unless explicitly requested.

Output schema must be JSON envelope with payload fields: summary, files_changed.
summary must include what was fixed and the validation command/result.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

pub fn merge_main_conflict_resolution_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-merge-main-conflict",
        body: r#"Intent: resolve merge conflicts after merging origin/main into this branch.

## Context
The pipeline ran `git merge origin/main` and there are conflicts in one or more files.
Read the evidence file in [knowledge_context] for failure details.

## Steps
1. Find all files with conflict markers (<<<<<<< / ======= / >>>>>>>).
2. Resolve each conflict by choosing the correct code or combining both sides.
3. Remove all conflict markers — no file should contain <<<<<<< when you are done.
4. Run the project's validation command to confirm everything passes.

## Autonomy
You have full autonomy. Fix the conflicts, commit your changes, and push. The pipeline is the safety net.
- Keep changes minimal — only resolve conflicts, do not refactor.
- Do NOT "fix" failures by weakening checks (for example changing coverage/lint/test ignore lists or lowering thresholds) unless explicitly requested.

Output schema must be JSON envelope with payload fields: summary, files_changed.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

pub fn ci_failure_remediation_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-ci-failure-remediation",
        body: r#"Your goal is to get this PR mergeable.

Read the evidence file in [knowledge_context] and resolve the issues it describes.

## Rules

- You have full autonomy. Fix the code, commit your changes, and push.
- Keep edits minimal and fix-focused; avoid unrelated refactors.
- Do NOT "fix" failures by weakening checks (e.g. changing ignore lists or lowering thresholds).
- Run the project's validation command locally to confirm your fixes pass before pushing.

Output schema must be JSON envelope with payload fields: summary, files_changed.
summary must include what was fixed and the validation command/result.
Return exactly one final envelope between <<GARDENER_JSON_START>> and <<GARDENER_JSON_END>>."#,
    }
}

#[allow(dead_code)] // wired up when post-merge validation creates a fix PR
fn post_merge_fix_template() -> PromptTemplate {
    PromptTemplate {
        version: "v1-post-merge-fix",
        body: r#"Intent: fix a post-merge validation failure on main.

The PR was merged successfully but validation on the combined main state fails.
Read the evidence file in [knowledge_context] for the validation error details.

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
    use super::{
        ci_failure_remediation_template, seeding_prompt_template, PromptRegistry,
        SEEDING_PROMPT_VERSION_SEEDING,
    };
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
    fn merge_remediation_template_grants_autonomy() {
        let registry = PromptRegistry::v1();
        let tpl = registry
            .template_for(WorkerState::Merging)
            .expect("template exists");
        assert_eq!(tpl.version, "v1-merge-remediation");
        assert!(tpl.body.contains("full autonomy"));
        assert!(!tpl.body.contains("Do NOT run git push"));
    }

    #[test]
    fn ci_failure_remediation_template_references_knowledge_context() {
        let tpl = ci_failure_remediation_template();
        assert_eq!(tpl.version, "v1-ci-failure-remediation");
        assert!(tpl.body.contains("[knowledge_context]"));
        assert!(tpl.body.contains("evidence file"));
        assert!(tpl.body.contains("<<GARDENER_JSON_START>>"));
    }

    #[test]
    fn gitting_remediation_template_prohibits_git_moves() {
        let registry = PromptRegistry::v1();
        let tpl = registry
            .template_for(WorkerState::Gitting)
            .expect("template exists");
        assert_eq!(tpl.version, "v1-gitting-remediation");
        assert!(tpl.body.contains("Do NOT run git push, git commit"));
        assert!(tpl.body.contains("git status"));
        assert!(!tpl.body.contains("<<GARDENER_JSON_START>>"));
        assert!(!tpl.body.contains("Output schema must be JSON envelope"));
    }

    #[test]
    fn seeding_prompt_template_uses_canonical_version_constant() {
        let tpl = seeding_prompt_template();
        assert_eq!(tpl.version, SEEDING_PROMPT_VERSION_SEEDING);
    }
}
