# Seeding System Overhaul

## Overview

Replace the broken seeding system with one that actually works. The current implementation sends a 2-sentence prompt with two numbers to an agent and hopes for useful tasks. The new system should have the agent inspect the entire repo, read the quality grades doc, read the codex agent team article's principles, and produce 10 high-quality tasks that both make the repo more agent-hospitable AND improve the quality score.

## Current State

**Files**: `tools/gardener/src/seeding.rs`, `tools/gardener/src/seed_runner.rs`, `tools/gardener/src/startup.rs`

The entire seed prompt is:
```
Seed backlog tasks for primary_gap={} with readiness_score={}.
Use evidence:
{quality_doc}
```

Problems:
1. **Prompt is content-free** — No instructions about what makes a good task, no output format spec, no guardrails
2. **No repo inspection** — Agent never reads the codebase; seeds blindly from two numbers
3. **No agent-hospitality framing** — Nothing tells the agent to think about making the repo better for agents
4. **No output schema** — `output_schema: None` in AdapterContext; agent must guess the JSON shape
5. **Rationale discarded** — `SeedTask.rationale` is parsed then dropped at `startup.rs:339`
6. **Single scope key** — All seeded tasks get `scope_key: primary_gap`, losing per-task categorization
7. **All P1 priority** — No differentiation between urgent and nice-to-have tasks
8. **One-shot only** — Gate requires `existing_backlog_count == 0`; never re-seeds
9. **Quality doc is garbage** — Single hardcoded `"core"` domain, crude file-count scoring
10. **Fallback is useless** — Three cycling template strings with no domain reasoning

## Desired End State

A seeding system where:
- The agent is given a rich structured prompt explaining what gardener needs
- The agent reads the repo structure, existing tests, CLAUDE.md/AGENTS.md, quality grades, and tech debt
- Tasks are categorized by domain, prioritized by impact, and come with rationale stored in the backlog
- The quality grades doc is real (done — committed separately) and drives task generation
- Re-seeding can happen when the backlog empties after completing all tasks

## Design Principles (from the Codex Agent Team article)

The seeding system should embody these principles from the article:
- **Repository knowledge is the system of record** — The seed prompt should point the agent at structured docs, not dump raw text
- **Progressive disclosure** — Give the agent a map (AGENTS.md, quality-grades.md, docs/ index) not a manual
- **Agent legibility** — Tasks should be concrete enough that a worker agent can pick them up without human clarification
- **Entropy/garbage collection** — Some seeded tasks should be cleanup/debt tasks, not just new features
- **Enforcing architecture and taste** — Tasks should include adding guardrails, linters, structural tests

## Plan

### Phase 1: Structured Seed Prompt

**File**: `tools/gardener/src/seeding.rs`

Replace `build_seed_prompt` with a proper structured prompt that:

1. **System context**: Explains what gardener is, what the seeding phase does, and what a good backlog task looks like
2. **Repo map**: Includes the AGENTS.md/CLAUDE.md content (or a pointer to read it), the docs/ directory listing, and the quality-grades.md
3. **Task requirements**: Specifies that tasks should:
   - Be concrete and actionable (not "improve testing")
   - Target a specific domain from the quality grades table
   - Include a priority (P0/P1/P2) based on impact
   - Balance agent-hospitality improvements with quality score improvements
   - Include at least 2 tasks for the `primary_gap` dimension
   - Include at least 2 cleanup/debt tasks
4. **Output schema**: Explicit JSON schema for the expected response format
5. **Few-shot example**: One example task showing the expected level of detail

The prompt should be a template in `prompt_registry.rs` or a dedicated file, not a `format!()` string.

**Changes**:
- `seeding.rs`: Replace `build_seed_prompt` with `build_seed_prompt_v2` that assembles context sections (similar to how `prompts.rs:render_state_prompt` works for workers)
- `seeding.rs`: Add a `SeedPromptContext` struct that collects all the inputs (profile, quality_doc, agents_md, docs_listing, tech_debt_items)
- `seeding.rs`: Read AGENTS.md/CLAUDE.md from the repo root and include in context

### Phase 2: Output Schema and Task Enrichment

**Files**: `tools/gardener/src/seed_runner.rs`, `tools/gardener/src/startup.rs`

1. **Add output schema to AdapterContext** — Set `output_schema: Some(seed_task_schema())` so the adapter can enforce structured output. Define the schema in `seed_runner.rs`.

2. **Expand SeedTask** — Add fields that map to `NewTask`:
   ```rust
   pub struct SeedTask {
       pub title: String,
       pub details: String,
       pub rationale: String,
       pub domain: String,       // maps to scope_key
       pub priority: String,     // "P0" | "P1" | "P2"
   }
   ```

3. **Store rationale** — Add a `rationale` column to the backlog SQLite schema (or store it in `details` with a separator). Update `startup.rs` task insertion to use `task.domain` as `scope_key` and parse `task.priority` into `Priority`.

4. **Store domain as scope_key** — Instead of all tasks sharing `primary_gap` as scope_key, use the agent-specified domain.

### Phase 3: Quality Domain Catalog (real domain discovery)

**Files**: `tools/gardener/src/quality_domain_catalog.rs`, `tools/gardener/src/quality_evidence.rs`, `tools/gardener/src/quality_scoring.rs`

Replace the hardcoded `["core"]` domain with actual domain discovery. The domains map to the logical groupings already identified in the quality-grades.md:

1. **`discover_domains`** — Scan the `src/` directory structure and map file groups to domains:
   - `triage*.rs` → "triage"
   - `backlog*.rs`, `priority.rs`, `task_identity.rs` → "backlog"
   - `seeding.rs`, `seed_runner.rs` → "seeding"
   - `worker*.rs`, `fsm.rs` → "worker-pool"
   - `agent/*.rs`, `protocol.rs`, `output_envelope.rs` → "agent-adapters"
   - `tui.rs`, `hotkeys.rs` → "tui"
   - `quality*.rs` → "quality-grades"
   - `startup.rs`, `worktree_audit.rs`, `pr_audit.rs` → "startup"
   - `git.rs`, `gh.rs`, `worktree.rs` → "git-integration"
   - `prompt*.rs` → "prompts"
   - `learning_loop.rs`, `postmerge_analysis.rs`, `postmortem.rs` → "learning"
   - Everything else → "infrastructure"

2. **`collect_evidence`** — Per-domain: count source files, test files, inline test modules, integration test files, instrumentation calls. This is richer than the current binary tested/untested classification.

3. **`score_domains`** — Per-domain scoring that considers:
   - Percentage of files with inline test modules
   - Whether a dedicated integration test file exists
   - Instrumentation coverage (from the existing linter data)
   - Whether the domain has structured docs

### Phase 4: Re-seeding Support

**Files**: `tools/gardener/src/startup.rs`, `tools/gardener/src/backlog_store.rs`

1. **Change the seeding gate** — Instead of `existing_backlog_count == 0`, use `count_active_tasks() == 0` (where "active" means pending or in-progress, not completed/failed). This allows re-seeding when all tasks have been processed.

2. **Add a `last_seeded_at` timestamp** — Store in a metadata table or the config cache. Prevent re-seeding more than once per run even if tasks complete during the run.

3. **Seed generation number** — Track a monotonically increasing generation number so re-seeded tasks can be distinguished from original seeds. Store as `source: "seed_runner_v2_gen_{n}"`.

### Phase 5: Kill the Fallback

**File**: `tools/gardener/src/startup.rs`

The 3-template fallback (`fallback_seed_tasks`) exists because seeding was unreliable. With a proper prompt and output schema, it should rarely fire. But rather than remove it entirely:

1. **Make fallback tasks quality-grade-driven** — Instead of 3 hardcoded strings, generate fallback tasks from the quality grades doc: one task per domain graded C or below, titled "Improve {domain} from {grade} to {target_grade}".

2. **Log when fallback fires** — Treat fallback as a signal that the seed prompt or model needs attention. Emit a warning-level log.

## Success Criteria

- [ ] Seed prompt is at least 500 words with structured sections (context, requirements, schema, example)
- [ ] Agent is instructed to inspect repo structure before generating tasks
- [ ] Seeded tasks have per-task domain (scope_key) and priority
- [ ] Rationale is stored in the backlog, not discarded
- [ ] Quality domain catalog discovers real domains (not just "core")
- [ ] Quality scoring uses per-domain file/test/integration counts
- [ ] Re-seeding fires when all active tasks are complete
- [ ] Fallback tasks are derived from quality grades, not hardcoded
- [ ] All existing tests continue to pass
- [ ] New unit tests for `build_seed_prompt_v2`, domain discovery, and re-seeding gate

## File Change Summary

| File | Change |
|------|--------|
| `src/seeding.rs` | Replace `build_seed_prompt` with `build_seed_prompt_v2` + `SeedPromptContext` |
| `src/seed_runner.rs` | Expand `SeedTask` struct, add output schema function |
| `src/startup.rs` | Use per-task domain/priority, store rationale, change seeding gate, improve fallback |
| `src/quality_domain_catalog.rs` | Real domain discovery from file structure |
| `src/quality_evidence.rs` | Per-domain evidence collection (test modules, integration tests, instrumentation) |
| `src/quality_scoring.rs` | Multi-dimension per-domain scoring |
| `src/backlog_store.rs` | Add `rationale` column (migration), update `NewTask` |
| `src/prompt_registry.rs` | Add `seeding-v2` prompt template (optional — may inline in seeding.rs) |
| `tests/phase03_startup.rs` | Update tests for new seeding behavior |
| New: unit tests | Tests for prompt assembly, domain discovery, scoring, re-seeding gate |

## Risks

- **Schema migration** — Adding `rationale` column to SQLite requires a migration. Use `ALTER TABLE tasks ADD COLUMN rationale TEXT DEFAULT NULL` which is safe for existing rows.
- **Prompt length vs context** — The seed prompt with full AGENTS.md + quality doc + docs listing could be long. Keep it under 4000 tokens by using pointers ("read AGENTS.md at the repo root") rather than inlining everything, but inline the quality grades since that's the primary input.
- **Re-seeding loop** — If the agent keeps generating tasks that immediately fail, re-seeding could loop. Mitigate with generation tracking and a max-generations cap.
