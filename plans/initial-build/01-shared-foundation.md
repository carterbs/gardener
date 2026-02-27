# Rust Gardener Orchestrator Cutover Plan

Updated: 2026-02-27 - Expanded with phase-by-phase hardening details pulled from `2026-02-26-agent-orchestrator-rust.md` and adapted to gardener naming/sandbox policy.
Updated: 2026-02-27 - Added selected Gastown-inspired safeguards: redundant completion observers, immutable-events plus state-cache split, typed internal protocol, worker identity/session/sandbox boundaries, done-means-gone teardown, TUI Problems view, and event-driven startup dispatch.
Updated: 2026-02-27 - Integrated quality-grade creation into Gardener runtime: profile/inference-driven domain discovery, deterministic scoring, and startup/backlog coupling.
Updated: 2026-02-27 - Added explicit prompt-layer contract: deterministic context assembly, state-scoped prompt packets, versioning, and observability.
Updated: 2026-02-27 - Clarified adapter/runtime policy: Claude Code binary (not SDK), permissive V1 execution mode for both adapters, agent-driven backlog seeding, and configurable validation command contract.
Updated: 2026-02-27 - Closed implementer gaps: concrete adapter CLI contracts, structured output extraction envelope, request-order scheduler queue model, identity hash inputs, V1 domain detectors, operational defaults, DI seams, and phase dependency clarifications.
Updated: 2026-02-27 - Added working-directory scope contract: explicit `--working-dir`/config support, repo-root default resolution, and scoped quality/backlog/validation behavior.
Updated: 2026-02-27 - Added execution-blocker contracts: fixture catalog, Phase 3->6 seeding handoff, stale-threshold policy, token trimming rules, knowledge ranking, scope-key derivation, profile schema, and meaningful-commit definition.
Updated: 2026-02-27 - Added remaining execution contracts: prune-only phase behavior, mock-bin argv/output contract, test-mode capability probe policy, fixture git isolation, learning-loop rule set, Claude stream/envelope parsing policy, post-merge validation-failure handling, and FakeTerminal assertion API.
Updated: 2026-02-27 - Added Codex deterministic parsing contract: `codex exec --json` JSONL lifecycle parsing, `--output-schema` support, terminal-state semantics, and app-server protocol probe for future migration.
Updated: 2026-02-27 - Added Phase 02b (Repo Triage + First-Run Interview): RepoIntelligenceProfile schema, CodexReadiness derived fields, triage config section, triage modules in crate layout, phase02b fixture contract, first-run detection gate in startup sequence, and profile consumption contract for quality grade generation and backlog seeding.
Updated: 2026-02-27 - Added agent detection and default model mapping: [agent] config table with single-agent simplicity path, per-task complexity tiers, recommended model mapping per agent, Codex per-repo config contract (.codex/config.toml + AGENTS.override.md multi-level traversal + 32 KiB cap), and --agent CLI flag.

## Overview
Replace `scripts/ralph` with a Rust orchestrator under `tools/gardener` that is resilient to git/worktree drift, backlog/PR collisions, and hung worker loops, while preserving the Gardener workflow and enforcing 100% line coverage for the new orchestrator code.

## Informed Understanding
- Current orchestration is TypeScript-first and centered in `scripts/ralph/index.ts` with worker scheduling + `Promise.race` coordination (`scripts/ralph/index.ts:1361`, `scripts/ralph/index.ts:1453`, `scripts/ralph/index.ts:1765`).
- Worktree and branch lifecycle handling lives in best-effort wrappers that often swallow errors (`scripts/ralph/git.ts:4`, `scripts/ralph/git.ts:68`, `scripts/ralph/git.ts:83`).
- Backlog/triage state is markdown-based and mutable in-repo (`scripts/ralph/backlog.ts:7`, `scripts/ralph/backlog.ts:8`, `scripts/ralph/backlog.ts:285`).
- Log-based sync expects merge events not consistently emitted, creating backlog drift risk (`scripts/ralph/backlog.ts:367`, `scripts/ralph/backlog.ts:410`, `scripts/ralph/types.ts:105`, `scripts/ralph/index.ts:582`).
- Existing state machine is implicit in flow control and not represented as a typed durable FSM (`scripts/ralph/index.ts:614`, `scripts/ralph/index.ts:742`, `scripts/ralph/index.ts:1032`, `scripts/ralph/index.ts:1205`).
- Repo standards require Rust-first orchestration tooling with thin shell wrappers (`AGENTS.md:17`, `AGENTS.md:18`, `docs/conventions/workflow.md:58`, `docs/conventions/workflow.md:60`).
- Worktree usage is mandatory for changes (`docs/conventions/workflow.md:5`, `docs/conventions/workflow.md:7`).
- Quality grades are already a first-class artifact (currently `docs/quality-grades.md`), and architecture lint already validates freshness expectations (`scripts/update-quality-grades.ts:21`, `tools/arch-lint/src/checks/quality_grades_freshness.rs:12`).

## Current State Analysis

### Runtime/Code Risks
- Task acquisition is triage-first then backlog, but no explicit priority taxonomy exists (`scripts/ralph/index.ts:1478`).
- Outstanding PR import skips branches attached in `activeWorktrees`, but `activeWorktrees` may track synthetic branch names before real branch reuse decisions are made (`scripts/ralph/index.ts:1637`, `scripts/ralph/index.ts:1734`, `scripts/ralph/index.ts:573`).
- Review fix loop does not gate commit/push on fix-step success in all paths (`scripts/ralph/index.ts:1105`, `scripts/ralph/index.ts:1118`, `scripts/ralph/index.ts:1172`).
- Codex adapter creates temp dirs and only removes output files, risking disk clutter (`scripts/ralph/agent.ts:308`, `scripts/ralph/agent.ts:482`).
- `--config` handling currently assumes directory semantics and appends `ralph.config.json` (`scripts/ralph/config.ts:32`, `scripts/ralph/config.ts:69`, `scripts/ralph/config.ts:72`).

### Live Operational Snapshot (2026-02-27)
- `git worktree list --porcelain` shows 66 attached worktrees, 51 under `/private/tmp/brad-os-ralph-worktrees/*`.
- `gh pr list --state open` shows one open PR (`#54`) on `harness-improvement-112`, while recent merged PRs include `#50` on the same branch naming lineage.
- `scripts/ralph/merge-conflicts.md` contains 5 unresolved conflict entries (`scripts/ralph/merge-conflicts.md:1`).
- Working tree currently shows mutable tracked task files (`git status --short`: `scripts/ralph/backlog.md`, `scripts/ralph/triage.md` modified), reinforcing the collision risk of source-controlled backlog state.

### Requirements Alignment Gaps
- Required startup quality-grade guard and bootstrap behavior are not explicit today (`scripts/ralph/orchestrator-requirements.md:7`).
- Required UNDERSTAND -> conditional PLANNING/DOING state model is not explicit today (`scripts/ralph/orchestrator-requirements.md:13`).
- Required worker request-order dispatch on explicit priorities is not implemented today (`scripts/ralph/orchestrator-requirements.md:10`).

## Desired End State
- A Rust binary orchestrator (`brad-gardener`) under `tools/gardener` fully replaces TypeScript `scripts/ralph` runtime.
- Central backlog uses durable, concurrent-safe storage with explicit `P0/P1/P2` priorities and lease-based assignment.
- Worker lifecycle is a typed state machine with durable transitions: `UNDERSTAND`, conditional `PLANNING`, `DOING`, `GITTING`, `REVIEWING`, `MERGING`, `COMPLETE`.
- Worker dispatch is highest-priority first, FIFO by `last_updated` within each priority, and in the order workers request work.
- Startup behavior:
  - On first run (no repo intelligence profile at `triage.output_path`): hard stop with actionable error directing the user to run `brad-gardener --triage-only` in a terminal. Triage requires a human; there is no automated fallback.
  - Check configured quality-grade output document (`quality_report.path`, default `docs/quality-grades.md`).
  - If missing: attempt bootstrap generation using repo intelligence profile as input context; if bootstrap cannot complete, enqueue/execute a `P0` infra-repair task.
  - Else use existing backlog.
  - If backlog empty, ask an agent to research the repository and seed tasks using quality-grade evidence, repo intelligence profile (codex_readiness dimensions), plus simplification/agent-legibility principles.
- Orchestrator can run `N` workers; each worker can launch `claude` (Claude Code binary) or `codex` via a pluggable adapter interface with per-state backend+model config.
- Orchestrator can run against a repository root or a scoped subdirectory via explicit working-directory configuration.
- Terminal UI shows worker state and friendly tool-call stream.
- JSONL logging is retained with bounded disk usage (defaults keep total <=50MB).
- Merge behavior defaults to merge-to-main, configurable.
- New orchestrator code is at 100% line coverage with enforced gates.

## Priority Model (Explicit)
- `P0`:
  - Configured repository validation command fails (Brad OS default: `npm run validate`).
  - Backlog task has related unmerged PR/open conflict state.
  - Merge-conflict/hung worktree recovery tasks.
  - Missing quality-grades infrastructure/doc bootstrap.
  - Any task blocking further autonomous progress (scheduler deadlock, adapter hard-fail, auth breakage).
- `P1`:
  - New feature implementation.
  - High-leverage harness/tooling improvements that increase agent throughput but are not active outages.
- `P2`:
  - Opportunistic cleanup and simplification tasks.
  - Refactors/documentation hygiene not currently blocking flow.
- Tie-breaker within same priority: oldest `last_updated` first (FIFO by update time).

## Technical Decisions (Hardened)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Queue persistence | SQLite in WAL mode at `.cache/gardener/backlog.sqlite` | Atomic claim/lease updates; crash-safe; no markdown race conditions. |
| DB access model | Single write actor + read-only connection pool | Preserves deterministic write ordering while avoiding read-path heartbeat starvation under N workers. |
| Concurrency model | Tokio async runtime + worker pull channel | Workers request work explicitly; scheduler can enforce request-order fairness. |
| Completion observability | Redundant observer chain (worker completion hook + scheduler sweep + startup reconciliation) | One failing observer cannot strand task state; all observers are idempotent. |
| State model | Immutable append-only event log + mutable projection caches | Preserves full auditability while keeping scheduler/TUI reads fast. |
| Internal protocol | Typed lifecycle/escalation events as Rust enums with JSON serialization | Eliminates stringly-typed transitions and ambiguous escalation semantics. |
| Agent execution model | Child-process adapters for Claude Code and Codex binaries | No SDK lock-in; runtime behavior matches existing CLI-native execution flows. |
| Codex machine I/O contract | Parse `codex exec --json` stdout JSONL only; treat `turn.completed` as success terminal and `turn.failed`/`error` as failure terminal; use `--output-schema` and `--output-last-message` when configured. | Gives deterministic control-flow for scheduler decisions without scraping human-oriented text output. |
| V1 permissions stance | Run both adapters in permissive mode first; harden sandbox later | Reduces initial cutover/debug surface and isolates reliability issues before sandbox tuning. |
| Prompt system | Versioned state templates + deterministic context assembler | Prevents prompt black-box behavior and makes prompt quality testable/reproducible. |
| Config format | Typed config loaded from file + CLI overrides | Supports per-state backend/model and safe runtime overrides. |
| Validation contract | Configurable repo validation command with auto-discovery fallback | Keeps Gardener portable while preserving strong pre-merge quality gates. |
| Quality-grade ownership | Gardener generates and refreshes a configurable quality-grade document path | Quality grades are an orchestrator primitive, not an external pre-step dependency. |
| Logging | `tracing` events + JSONL audit stream | Real-time UI updates plus machine-readable postmortem trace. |
| TUI stack | `ratatui` + `crossterm` with non-TTY fallback | Usable locally and in CI/piped contexts. |
| Worker identity model | Stable worker slot + per-attempt session + per-run sandbox identity | Prevents state bleed across retries and supports deterministic cleanup. |
| Completion teardown policy | Done-means-gone: completion always tears down session/sandbox/worktree | Prevents resource drift and zombie workers after merge. |
| Startup dispatch model | Event-driven startup gate (resume assigned > ready queue > pending signals > idle) | Avoids idle false-positives and ensures resumable work is always prioritized. |
| Worktree strategy | Explicit create/resume/cleanup/prune lifecycle | Prevents orphaned worktrees and stale branch drift. |
| Coverage gate | Dedicated gardener coverage command in CI + local validation | Enforces 100% line coverage on orchestrator code paths. |

## Architectural Decisions
- Quality grades path: configurable via `quality_report.path` (default `docs/quality-grades.md` for this repo).
- Backlog storage: canonical SQLite store at `.cache/gardener/backlog.sqlite` plus optional human-readable snapshot export `scripts/gardener/backlog.md` (generated, non-authoritative).
- PR data source: `gh` only.
- Recently-merged PR lookback default: 7 days (configurable), used for dedupe and stale-worktree reconciliation.
- Merge target behavior: `merge_to_main=true` default with config override.
- JSONL retention defaults: rotate per run and prune by total-size budget (50MB default), oldest-first.
- Quality-grade document ownership: quality-grade output is generated by Gardener's internal creator path; legacy external script usage is transitional fallback only during cutover.
- Validation command policy:
  - `validation.command` is configurable per repository/profile.
  - Brad OS default is `npm run validate`.
  - if not configured, Gardener may auto-discover a repo-native validation command or require agent proposal + review confirmation.
  - startup gate is controlled by `startup.validate_on_boot` (default `false` for portability).
- Working directory policy:
  - `scope.working_dir` is configurable; CLI override: `--working-dir <path>`.
  - default resolution: if current process cwd is inside a git repo, use repo root; otherwise use cwd.
  - when scoped to subdirectory, domain discovery/quality grading/seeding/context assembly operate on that subtree only.
  - git/gh operations still execute against the containing repository.

## Config Schema (Core Excerpt)

Most users only need to set `agent.default`. Gardener auto-detects the agent during triage and writes this for them. Everything else has sensible defaults derived from the chosen agent.

```toml
[orchestrator]
parallelism = 3
branch_prefix = "change"
worktree_dir = "/tmp/gardener-worktrees"
merge_to_main = true

[scope]
working_dir = "."  # resolved relative to process cwd; defaults to repo root when inside a repo

# ──────────────────────────────────────────────────────────────────────
# AGENT CONFIGURATION — written by triage; most users don't touch this
# ──────────────────────────────────────────────────────────────────────

[agent]
default = "claude"   # "claude" | "codex" — auto-detected by triage and confirmed by user
                     # drives backend for all tasks unless overridden per-task below
                     # model defaults come from Gardener's recommended mapping for this agent

# ── Optional per-task overrides ────────────────────────────────────────
# Most users: leave everything below this line commented out.
# Gardener applies its recommended model for each task tier automatically.
# Power users: uncomment and override any task's backend or model.
#
# [states.understand]
# backend = "claude"
# model = "claude-haiku-4-5-20251001"   # low-complexity: fast classification
#
# [states.planning]
# backend = "claude"
# model = "claude-sonnet-4-6"           # high-complexity: architectural reasoning
#
# [states.doing]
# backend = "claude"
# model = "claude-sonnet-4-6"           # high-complexity: multi-file changes
#
# [states.gitting]
# backend = "claude"
# model = "claude-haiku-4-5-20251001"   # low-complexity: commit and push
#
# [states.reviewing]
# backend = "claude"
# model = "claude-sonnet-4-6"           # medium-complexity: PR review
#
# [states.merging]
# backend = "claude"
# model = "claude-haiku-4-5-20251001"   # low-complexity: rebase and merge
#
# [seeding]
# backend = "claude"
# model = "claude-sonnet-4-6"           # medium-complexity: task ideation
# max_turns = 12
# ───────────────────────────────────────────────────────────────────────

[quality_report]
path = "docs/quality-grades.md"
stale_after_days = 7
stale_if_head_commit_differs = true

[startup]
validate_on_boot = false
validation_command = "npm run validate"  # written by triage from Q4 answer

[validation]
command = "npm run validate"
allow_agent_discovery = true

[execution]
permissions_mode = "permissive_v1"       # permissive_v1 | restricted
worker_mode = "normal"                   # normal | stub_complete
test_mode = false                        # enables fixture/mock-bin compatibility behaviors

[scheduler]
lease_timeout_seconds = 900
heartbeat_interval_seconds = 15
starvation_threshold_seconds = 180
reconcile_interval_seconds = 30

[prompts.token_budget]
understand = 6000
planning = 9000
doing = 12000
gitting = 4000
reviewing = 10000
merging = 5000

[learning]
confidence_decay_per_day = 0.01
deactivate_below_confidence = 0.20

[triage]
output_path = ".gardener/repo-intelligence.toml"
stale_after_commits = 50
discovery_max_turns = 12
```

Model validity policy:
- `agent.default` is required if any `[states.*]` or `[seeding]` sections omit `backend`.
- placeholders such as `"..."`, `"TODO"`, or empty model strings are invalid.
- startup capability probe must validate all configured backend/model combinations before scheduler starts.
- in `execution.test_mode=true`, fixtures may use synthetic model aliases (`fixture-*`) that map to mock-bin adapters only.
- if both `agent.default` and an explicit `[states.X] backend` are set, the explicit value wins.

## V1 Operational Defaults (Normative)
- `scheduler.lease_timeout_seconds = 900`
- `scheduler.heartbeat_interval_seconds = 15`
- `scheduler.starvation_threshold_seconds = 180`
- `scheduler.reconcile_interval_seconds = 30`
- `prompts.token_budget.understand = 6000`
- `prompts.token_budget.planning = 9000`
- `prompts.token_budget.doing = 12000`
- `prompts.token_budget.gitting = 4000`
- `prompts.token_budget.reviewing = 10000`
- `prompts.token_budget.merging = 5000`
- `learning.confidence_decay_per_day = 0.01`
- `learning.deactivate_below_confidence = 0.20`
- `seeding.backend = "codex"`
- `seeding.model = "gpt-5-codex"`
- `seeding.max_turns = 12`
- `quality_report.stale_after_days = 7`
- `quality_report.stale_if_head_commit_differs = true`
- `triage.output_path = ".gardener/repo-intelligence.toml"`
- `triage.stale_after_commits = 50`
- `triage.discovery_max_turns = 12`

## Agent Default + Task Model Mapping (Normative)

When `agent.default` is set and no per-task override exists, Gardener applies its recommended model for that agent based on the task's complexity tier.

### Task Complexity Tiers

| Task | Tier | Rationale |
|------|------|-----------|
| `triage.discovery` | medium | reads files, reasons about quality — needs capability but not heavy reasoning |
| `seeding` | medium | task ideation from evidence — capable generalist |
| `states.understand` | low | classification only — fast and cheap wins |
| `states.planning` | high | architectural reasoning, multi-file scope design |
| `states.doing` | high | multi-file implementation, many turns |
| `states.reviewing` | medium | PR review and feedback — capable generalist |
| `states.gitting` | low | commit, push, branch ops — deterministic steps |
| `states.merging` | low | rebase, conflict resolution — deterministic steps |

### Recommended Model Mapping

The mapping below represents Gardener's opinionated defaults. Brad OS uses these unless a `[states.*]` override is present. Update this table as model capabilities and cost profiles evolve.

**Claude:**

| Tier | Recommended model | Notes |
|------|------------------|-------|
| low | `claude-haiku-4-5-20251001` | Fastest, cheapest; sufficient for classification and mechanical ops |
| medium | `claude-sonnet-4-6` | General capable; good cost/quality balance for most tasks |
| high | `claude-sonnet-4-6` | Default high tier; upgrade to `claude-opus-4-6` for planning if quality matters more than cost |

**Codex:**

| Tier | Recommended model | Notes |
|------|------------------|-------|
| low | `gpt-5-codex` | Single model; no tier differentiation in V1 |
| medium | `gpt-5-codex` | — |
| high | `gpt-5-codex` | — |

Model IDs are validated at startup against the capability probe. Invalid or unavailable model IDs fail fast with actionable diagnostics.

### Codex Per-Repo Config Contract (Normative)

Codex uses a different config layout than Claude. Gardener's agent detection and discovery prompt must account for both.

- **Primary instruction file:** `AGENTS.md` — shared with Claude, read by both agents.
- **Override file:** `AGENTS.override.md` — Codex-specific; takes precedence over `AGENTS.md` at the same level. Presence is a strong Codex signal during agent detection.
- **Named variants:** `AGENTS.<name>.md` and `AGENTS.<name>.override.md` — selected via `codex --agents <name>`.
- **Per-repo config:** `.codex/config.toml` — present at any directory level; Codex walks from repo root → CWD and loads all, closest wins on key conflicts.
- **Global config:** `~/.codex/` (config.toml, AGENTS.md, rules/).
- **Multi-level traversal:** Codex walks root → CWD at each level, reading `AGENTS.override.md` → `AGENTS.md` → fallback names. Concatenated content is capped at 32 KiB (`project_doc_max_bytes`). Content beyond 32 KiB is silently truncated — a critical quality signal.
- **Fallback filenames:** configurable via `project_doc_fallback_filenames` in `~/.codex/config.toml`. Gardener does not attempt to read global user config; it scans for standard filenames only.

**32 KiB cap implication for quality assessment:** At ~500–700 lines of markdown, combined AGENTS.md content across all levels hits the Codex cap. Monolithic or multi-level AGENTS.md that exceeds this gets silently cut off mid-instruction. This is a concrete, measurable quality defect that the discovery agent must flag.

## Quality Grade Creation (Integrated, Portable)
- Gardener owns initial creation and subsequent refresh of configured quality-grade output (`quality_report.path`).
- Domain set is repository-specific and discovered at runtime (or via profile), not hardcoded into Gardener binary logic.
- Domain discovery contract:
  - optional profile file supplies explicit domain definitions and mapping rules.
  - without profile, detectors infer domains from architecture docs, path topology, ownership metadata, and test/code clustering.
  - each discovered artifact maps to exactly one discovered domain or explicit `shared` bucket.
  - unmapped artifacts generate a deterministic `P0` infra task to fix domain mapping drift.
- Scoring contract (deterministic, no LLM dependency):
  - coverage strength (primary baseline),
  - test quality (assertion density adjustment),
  - untested file risk weighting (high-risk pathways degrade),
  - API/iOS completeness modifiers.
- Output contract (unified readiness-first document):
  - stable markdown structure with `Last updated: YYYY-MM-DD` and generation metadata (profile path, generator version),
  - `# Repo Quality Report` headline with readiness score and grade (e.g., `Readiness: 65/100 (C) | Primary gap: knowledge_accessible`),
  - `## Triage Baseline` section: profile path, `readiness_score`, `readiness_grade`, `primary_gap` from loaded profile,
  - `## Agent Readiness` section: per-dimension table (agent steering, knowledge accessible, mechanical guardrails, local feedback loop, coverage signal) with status and score; populated from profile `[codex_readiness]`,
  - `## Coverage Detail` section as evidence for the "coverage signal" dimension:
    - per-domain grade table containing all discovered non-shared domains,
    - evidence sections for tested files, untested files, and prioritized debt items.
- Startup coupling:
  - if quality grades missing or stale beyond policy threshold, Gardener refreshes them before normal scheduling.
  - if refresh fails, Gardener records evidence and enqueues/executes `P0` infra-repair task.
- Backlog coupling:
  - backlog seeding is performed by an agent with quality-grade evidence + repository context + codex-article principles as input.
  - seed generation is intentionally non-deterministic; Gardener records full seed prompt/context and resulting tasks for audit/replay.
  - ranking heuristics still favor low-grade/high-risk areas, but task ideation itself is agent-driven.

## Domain Discovery Algorithm (V1, Deterministic)
- Domain definition (V1):
  - a domain is a stable product capability bucket with a slug key (e.g., `cycling`, `meal-planning`).
  - each non-generated artifact must map to exactly one domain slug or `shared`.
- Discovery order is strict and short-circuited:
  1. Load explicit domain map from profile (if provided).
  2. Parse `docs/architecture/*.md` headings for domain slugs.
  3. Infer domains from first-level product directories under `packages/functions/src/` and `ios/BradOS/Features/`.
  4. Add `shared` bucket for cross-cutting artifacts.
- Artifact mapping rules (first match wins):
  - explicit profile path glob -> domain
  - architecture-doc mapping table -> domain
  - directory-prefix mapping (`handlers/<domain>`, `services/<domain>`, etc.) -> domain
  - unmatched files -> `unmapped` list (never silently dropped)
- Drift policy:
  - if `unmapped` non-generated files > 0, quality generation returns typed failure and enqueues `P0` infra task.
  - output includes deterministic unmapped evidence list sorted by path.
- Scope rule:
  - discovery and mapping inspect only files rooted at `scope.working_dir` (plus required global policy docs).

## Domain Profile Schema (Normative, Portable)
- Location default: `.gardener/domain-profile.toml` (override via config key `quality_report.profile_path`).
- Required fields:
  - `version` (integer, currently `1`)
  - `domains` array of `{ slug, display_name, include_globs[] }`
  - optional `shared_globs[]`
  - optional `artifact_overrides[]` with `{ path_glob, domain_slug }`
- Validation:
  - duplicate `slug` or overlapping explicit overrides fail startup with typed config error.
  - if profile exists, it is authoritative over path inference.

## Repo Intelligence Profile (First-Run Triage, Normative)
- Location default: `.gardener/repo-intelligence.toml` (override via config key `triage.output_path`).
- Committed to version control alongside `.gardener/domain-profile.toml`.
- Written by Phase 02b triage on first run; updated on `--retriage` or when stale.
- Staleness policy: profile is considered stale when `meta.head_sha` differs from current `git rev-parse HEAD` by more than `triage.stale_after_commits` commits. Stale profile triggers a non-blocking `WARN`; it does not block startup.
- Schema sections (all required except `[discovery.quality_docs]` which may be absent in new repos):
  - `[meta]` — schema_version (integer, currently `1`), created_at, head_sha, working_dir, synthesis_used.
  - `[discovery.steering]` — agents_md_present, agents_md_line_count, agents_md_style ("pointer"|"monolith"|"mixed"|"absent"), claude_md_present, dot_claude_dir_present, skills_dir_present, other_steering_doc_paths.
  - `[discovery.architecture]` — architecture_md_present, architecture_doc_paths, design_doc_paths.
  - `[discovery.conventions]` — contributing_md_present, convention_doc_paths, editorconfig_present, other_style_config_paths.
  - `[discovery.testing]` — test_framework_detected ("jest"|"vitest"|"pytest"|"cargo_test"|"go_test"|"none"|"multiple"), source_file_count, test_file_count, coverage_config_detected, coverage_summary_path, ci_test_step_detected.
  - `[discovery.linting]` — standard_linters, custom_linters, pre_commit_hooks_present, ci_lint_step_detected.
  - `[discovery.quality_docs]` — quality_grades_doc_present, quality_grades_path, exec_plans_dir_present, references_dir_present.
  - `[user_validated]` — architecture_coverage, conventions_coverage, test_coverage_grade, guardrails_grade, agent_steering_grade, validation_command, additional_context, corrections_made, validated_at.
  - `[codex_readiness]` — boolean flags for each Codex article dimension plus derived `readiness_score` (0–100), `readiness_grade` (A–F), `primary_gap` (dimension slug with highest weight that is unmet).
- `codex_readiness` flags are computed deterministically from `discovery.*` + `user_validated.*`; no LLM involvement.
- Quality grade generation (Phase 03) reads this profile: `user_validated.validation_command` overrides startup config; `[codex_readiness]` scores populate the `## Agent Readiness` section of the unified quality document; `primary_gap` is included in the seeding agent prompt.
- Schema version mismatch returns typed `TriageError::SchemaMismatch` without panic; startup emits actionable error with upgrade instructions.
- Non-interactive hard stop: if non-interactive environment is detected (`CLAUDECODE` env var, `CODEX_THREAD_ID` env var, `CI` env var, or non-TTY stdin), triage refuses to run and emits: "Triage requires a human and cannot run non-interactively. Run `brad-gardener --triage-only` in a terminal."

## Quality Scoring Rubric (V1, Deterministic)
Two distinct scoring formulas apply; both are deterministic and produce reproducible output.

**Readiness dimension scoring** (computed during triage, stored in profile `[codex_readiness]`, read back by quality-grade generator):
- Grade-to-score mapping: A=90, B=70, C=45, D=25, F=0, unknown=10.
- Each of the five dimensions carries equal weight=20.
- `readiness_score = sum(dimension_scores)` clamped 0–100.
- Quality grade generator does not recompute this; it reads pre-computed values from the profile.

**Domain coverage scoring** (computed at quality-grade generation time; populates `## Coverage Detail` section):
- Score range is `0..100`, mapped to grade:
  - `A` >= 90
  - `B` >= 80
  - `C` >= 70
  - `D` >= 60
  - `F` < 60
- Domain score formula:
  - `score = coverage_component + test_presence_component + risk_penalty + completeness_modifiers`
  - `coverage_component = min(60, coverage_pct * 0.60)` from primary coverage source
  - `test_presence_component = min(25, tested_files_ratio * 25)`
  - `risk_penalty = -(high_risk_untested * 6 + medium_risk_untested * 3 + low_risk_untested * 1)` clamped to `-35`
  - `completeness_modifiers = api_modifier + ios_modifier` where each is `+5` when complete, `0` when partial, `-5` when missing
  - final score is clamped `0..100`
- Tie-break ordering in reports/backlog seeding:
  - lower score first, then higher high-risk untested count, then domain slug ascending.

## Coverage Source Contract (V1)
- Primary coverage input: `packages/functions/coverage/coverage-summary.json` (same source currently used by `scripts/update-quality-grades.ts`).
- Optional secondary inputs:
  - Rust module coverage from `cargo llvm-cov --json` when Rust domains are present.
  - iOS test presence is inventory-based in V1 (count + critical-path checks), not line-coverage-weighted.
- If no primary coverage source exists, quality generation does not guess; it emits a `P0` infra task with the missing path and expected command.
- Scoped mode:
  - when `scope.working_dir` is a subdirectory, coverage/evidence are filtered to artifacts under that scope.

## Seeding Principles Source (Codex Article, Normalized)
- Backlog seeding prompts must encode these explicit principles from `docs/references/codex-agent-team-article.md`:
  - prioritize agent legibility (small, explicit, enforceable tasks)
  - prefer deterministic checks and mechanical validation gates
  - preserve repository-local knowledge as source of truth
  - raise architectural clarity over one-off tactical edits
- Seeding output format is fixed to high-level tasks containing:
  - title
  - rationale
  - expected validation signal
  - suggested priority (`P0`/`P1`/`P2`)
  - bounded scope notes.

## Seeding Prompt Contract (V1)
- Seeding runner always sends:
  - objective header,
  - quality-grade table + evidence excerpts,
  - architecture/domain summary,
  - repo intelligence profile summary (if present): `codex_readiness` dimension scores + `user_validated.additional_context` + `primary_gap` — seeding agent must treat the `primary_gap` dimension as the highest-leverage area for new tasks,
  - explicit priority rubric (`P0/P1/P2`),
  - response schema requirement.
- Seeding response schema (strict JSON envelope payload):
  - `{ "tasks": [{ "title": "...", "rationale": "...", "priority": "P0|P1|P2", "validation_signal": "...", "scope_notes": "..." }] }`
- Task count limits:
  - min `3`, max `12` tasks per seed run.
- Invalid seeding payload handling:
  - one automatic retry with schema reminder,
  - then emit `P0` infra task `seeding-output-invalid` with raw evidence artifact.

## Fixture Contract (Normative)
- All phase validation gates must use concrete fixtures under:
  - `tools/gardener/tests/fixtures/configs/*.toml`
  - `tools/gardener/tests/fixtures/repos/*`
  - `tools/gardener/tests/integration/mock_bins/*`
- Required config fixtures:
  - `phase01-minimal.toml`
  - `phase02-backlog.toml`
  - `phase02b-triage.toml`
  - `phase03-startup-seeding.toml`
  - `phase04-scheduler-stub.toml`
  - `phase05-fsm.toml`
  - `phase06-codex.toml`
  - `phase06-claude.toml`
  - `phase07-git-gh-recovery.toml`
  - `phase08-ui.toml`
  - `phase09-cutover.toml`
  - `phase10-full.toml`
- Mock-bin interface:
  - each fake binary must log argv and env to fixture artifacts,
  - return code and stdout/stderr behavior are scenario-driven via fixture env vars,
  - `git`/`gh` fakes must support minimal command matrix used by startup/reconcile/merge paths.
- Mock-bin argv contract (normative, V1):
  - `git` expected argv patterns:
    - `worktree list --porcelain`
    - `worktree add <path> <branch>`
    - `worktree remove <path> --force`
    - `branch -D <branch>`
    - `rev-parse HEAD`
    - `rev-list --count <base>..<branch>`
    - `merge-base --is-ancestor <sha> <target>`
  - `gh` expected argv patterns:
    - `pr list --state open --json number,headRefName,state`
    - `pr view <number> --json state,mergedAt,mergeable,mergeCommit`
  - `codex`/`claude` expected argv patterns:
    - codex: `exec --json --model <model> -C <cwd> ...`
    - claude: `-p "<prompt>" --output-format stream-json --verbose --model <model>`
  - each pattern has fixture-defined stdout/stderr + exit code; unknown argv must fail loudly.
- Scoped fixture repo minimum structure (`tools/gardener/tests/fixtures/repos/scoped-app/`):
  - `.git/` (initialized fixture repo)
  - `AGENTS.md`
  - `docs/architecture/` with at least 2 domain docs
  - `packages/functions/src/<domain>/` sample handler/service files
  - `packages/functions/coverage/coverage-summary.json`
  - optional `ios/BradOS/Features/<domain>/` sample files for iOS mapping tests
  - `docs/quality-grades.md` (stale + fresh variants for staleness tests)
- Triage fixture repos (`tools/gardener/tests/fixtures/repos/triage-*/`):
  - `triage-fully-equipped/` — AGENTS.md (pointer-style, <100 lines) + `ARCHITECTURE.md` + `tools/arch-lint/` + `.github/workflows/ci.yml` with coverage gate + `docs/quality-grades.md` + `docs/exec-plans/`.
  - `triage-minimal/` — only `README.md`; no AGENTS.md, no tests, no linters, no CI config.
  - `triage-agents-only/` — `AGENTS.md` (monolith style, >300 lines) but no test, lint, or CI signals.
  - `triage-no-agents/` — `jest.config.js` + `.eslintrc.json` + `.github/workflows/ci.yml` but no AGENTS.md or CLAUDE.md.
  - Each triage fixture repo has its own `.git/` (isolated per fixture git isolation contract).
  - `tests/fixtures/triage/mock-discovery-responses/<repo-slug>.json` — mock agent output_envelope responses for each fixture repo; loaded in `execution.test_mode = true`.
  - `tests/fixtures/triage/expected-profiles/<repo-slug>.toml` — expected profile output for each fixture repo (used in deterministic assertion tests).
- Fixture git isolation:
  - each fixture repo is an independent git repository with its own `.git`,
  - all integration tests must set cwd explicitly to the fixture repo root (never parent repo),
  - worktree temp paths must be fixture-scoped to prevent outer-repo contamination.

## Portability Requirements (Non-Negotiable)
- No hardcoded domain names in core Gardener logic.
- No hardcoded repository paths in core grading logic beyond configurable defaults.
- Domain inference and scoring providers must be pluggable so Gardener can run in unrelated repositories.
- Repository-specific mappings/rubrics live in profile/config files, not compiled code branches.
- If a repository cannot be confidently decomposed into domains, Gardener must fail with actionable diagnostics instead of silently producing partial grades.

## Proposed Crate Layout

```text
tools/gardener/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── config.rs
│   ├── errors.rs
│   ├── types.rs
│   ├── backlog_store.rs
│   ├── backlog_snapshot.rs
│   ├── priority.rs
│   ├── task_identity.rs
│   ├── triage.rs
│   ├── triage_discovery.rs
│   ├── triage_interview.rs
│   ├── repo_intelligence.rs
│   ├── startup.rs
│   ├── quality_grades.rs
│   ├── quality_domain_catalog.rs
│   ├── quality_evidence.rs
│   ├── quality_scoring.rs
│   ├── worktree_audit.rs
│   ├── pr_audit.rs
│   ├── seeding.rs
│   ├── scheduler.rs
│   ├── worker_pool.rs
│   ├── fsm.rs
│   ├── worker.rs
│   ├── worker_identity.rs
│   ├── protocol.rs
│   ├── state_cache.rs
│   ├── watchdog.rs
│   ├── prompts.rs
│   ├── prompt_context.rs
│   ├── prompt_registry.rs
│   ├── prompt_knowledge.rs
│   ├── learning_loop.rs
│   ├── postmerge_analysis.rs
│   ├── postmortem.rs
│   ├── git.rs
│   ├── gh.rs
│   ├── worktree.rs
│   ├── tui.rs
│   ├── logging.rs
│   └── log_retention.rs
└── tests/
    ├── backlog_store_test.rs
    ├── scheduler_test.rs
    ├── fsm_test.rs
    ├── protocol_test.rs
    ├── state_cache_test.rs
    ├── watchdog_test.rs
    ├── startup_test.rs
    ├── quality_grades_test.rs
    ├── quality_domain_catalog_test.rs
    ├── quality_scoring_test.rs
    ├── prompt_context_test.rs
    ├── prompt_registry_test.rs
    ├── learning_loop_test.rs
    ├── postmortem_test.rs
    ├── worktree_test.rs
    ├── adapter_contract_test.rs
    ├── tui_problems_test.rs
    ├── triage_test.rs
    ├── triage_discovery_test.rs
    ├── triage_interview_test.rs
    ├── repo_intelligence_test.rs
    └── integration/
        ├── full_pipeline_test.rs
        ├── concurrency_claims_test.rs
        ├── recovery_test.rs
        └── mock_bins/
```

## SQLite Backlog Schema (Canonical)

```sql
CREATE TABLE IF NOT EXISTS tasks (
  task_id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  details TEXT NOT NULL DEFAULT '',
  priority INTEGER NOT NULL,
  status TEXT NOT NULL,
  -- ready | leased | in_progress | complete | failed | parked
  last_updated TEXT NOT NULL,
  lease_owner TEXT,
  lease_expires_at TEXT,
  source TEXT NOT NULL,
  related_pr INTEGER,
  related_branch TEXT,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  last_event_id INTEGER NOT NULL DEFAULT 0,
  error_message TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS idx_tasks_queue
  ON tasks(status, priority ASC, last_updated ASC, created_at ASC);

CREATE TABLE IF NOT EXISTS worker_state (
  worker_id INTEGER PRIMARY KEY,
  state TEXT NOT NULL,
  task_id TEXT REFERENCES tasks(task_id),
  session_id TEXT,
  sandbox_id TEXT,
  last_event_id INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL,
  run_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  worker_id INTEGER,
  task_id TEXT,
  payload_json TEXT NOT NULL DEFAULT '{}'
) STRICT;

CREATE TABLE IF NOT EXISTS knowledge_entries (
  knowledge_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  -- success_pattern | failure_pattern | guardrail
  scope TEXT NOT NULL,
  -- global | domain:<name> | path:<prefix> | component:<id>
  summary TEXT NOT NULL,
  evidence_json TEXT NOT NULL,
  confidence REAL NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_applied_at TEXT,
  applies_count INTEGER NOT NULL DEFAULT 0,
  is_active INTEGER NOT NULL DEFAULT 1
) STRICT;
```

Atomic lease claim (single transaction):

```sql
UPDATE tasks
SET
  status = 'leased',
  lease_owner = ?1,
  lease_expires_at = ?2,
  last_updated = ?3
WHERE task_id = (
  SELECT task_id
  FROM tasks
  WHERE status = 'ready'
  ORDER BY priority ASC, last_updated ASC, created_at ASC
  LIMIT 1
)
RETURNING *;
```

Startup crash-recovery invariant:
- Before workers start, any `leased`/`in_progress` task with expired or missing lease must be reset to `ready` with incremented `attempt_count`.

## State Model: Immutable Events + Mutable Projections
- `events` is append-only and authoritative for lifecycle history.
- `tasks` and `worker_state` are projection tables optimized for scheduler and TUI reads.
- Projection rows are monotonic by `last_event_id`; stale/out-of-order projection updates must be rejected.
- On startup, a reconciliation pass validates projection consistency against events and repairs drift.

## Execution Profiles (V1 Permissive)
- `claude` runner profile:
  - Uses the Claude Code binary (`claude`) via child-process adapter (no Claude SDK dependency).
  - Non-interactive prompt execution from Rust with streamed event parsing.
  - Run in permission-bypass mode for V1 to avoid permission/sandbox blockers during cutover stabilization.
  - Command/flag details are centralized in adapter config and contract-tested.
- `codex` runner profile:
  - Uses `codex exec --json --dangerously-bypass-approvals-and-sandbox` for V1 parity/debuggability.
  - Output captured via JSON stream + output file for deterministic parsing.
  - Per-state turn limits enforced by Gardener worker FSM and retry budgets.
- Hardening plan:
  - sandbox tightening for both adapters is explicitly deferred to a post-cutover hardening phase.
  - permission profile is config-driven so future restricted modes do not require architecture changes.

## Adapter CLI Contract (Normative, V1)
- Capability probe runs at startup and writes `.cache/gardener/adapter-capabilities.json`.
- Probe captures:
  - binary path
  - version string
  - supported flags from `--help`
  - accepted invocation template
  - adapter-specific output-mode probe: Claude checks `--output-format` flag presence; Codex checks `--json`, `--output-schema`, and `--output-last-message` flag presence plus stdin prompt compatibility (`echo ... | codex ...`)
- Codex invocation template (required in V1):
  - `codex exec --json --dangerously-bypass-approvals-and-sandbox --model <model> -C <cwd> -o <output_file> -`
  - prompt is written to stdin
  - event stream parsed from stdout JSONL; final text prefers output file fallback.
- Claude invocation template (required in V1):
  - `claude -p "<prompt>" --output-format stream-json --verbose --model <model>`
  - prompt is passed as the argument to `-p`; **stdin must not be piped** (`Stdio::null()` in Rust) to avoid TTY hang in Ink.
  - stdout is parsed as newline-delimited JSON (NDJSON) line by line.
  - `AgentEvent` variants map directly to stream-json message types: `assistant` messages → `OutputText`/`ToolCall`, `result.usage` → `TurnComplete`, tool-result items → `ToolResult`.
  - structured envelope payload is extracted from the `result` field of the terminal `{"type":"result","subtype":"success","result":"..."}` NDJSON message; `<<GARDENER_JSON_START>>` / `<<GARDENER_JSON_END>>` markers are located within that string, not by scanning raw stdout.
  - set env `CLAUDECODE=""` to prevent recursive-invocation error if Gardener runs inside a Claude session.
- Claude optional flags:
  - `--max-turns` and any permission-bypass flags are only used when capability probe confirms support.
  - if unsupported, Gardener enforces turn/cycle limits at FSM layer and records capability downgrade event.
- Tool/sandbox flags:
  - Gardener does not depend on Claude-specific `--allowedTools`/sandbox flags in V1.
  - tool constraints are enforced by prompt contract + orchestrator policy + process sandbox/worktree boundaries.
- Startup fails fast with actionable diagnostics if required template cannot be executed for a configured backend.
- Capability diagnostics must include exact attempted argv and stderr tail.
- Flag detection strategy:
  - parse `--help` output with line-based regex `(^|\\s)(-{1,2}[A-Za-z0-9-]+)` and de-duplicate.
  - required flags for each template must be present exactly as tokens.
- Stdin compatibility probe success criteria (Codex only; Claude probe checks `--output-format` flag presence instead):
  - process exits `0` OR emits recognizable prompt-consumed output marker from mock/real backend contract.
  - in `execution.test_mode=true`, success may be driven by mock-bin marker env without strict CLI semantics.

`adapter-capabilities.json` schema (normative):
```json
{
  "version": 1,
  "generated_at": "ISO-8601",
  "adapters": {
    "codex": {
      "binary_path": "...",
      "version_text": "...",
      "supports_stdin_prompt": true,
      "supported_flags": ["--json", "--model", "-C", "-o", "..."],
      "accepted_template": "codex exec --json ..."
    },
    "claude": {
      "binary_path": "...",
      "version_text": "...",
      "supports_stdin_prompt": false,
      "supports_stream_json": true,
      "supported_flags": ["-p", "--output-format", "--verbose", "--model", "..."],
      "accepted_template": "claude -p \"<prompt>\" --output-format stream-json --verbose --model <model>"
    }
  }
}
```

## DB Write Actor Contract (Normative)
- Implementation shape is fixed:
  - dedicated Tokio task owns one writable SQLite connection,
  - API is `tokio::mpsc::Sender<WriteCmd>` + `oneshot` response per command,
  - all state mutations and claims flow through this actor.
- Read path:
  - separate read-only connection pool (no writes) for scheduler/tui/status queries.
- `Arc<Mutex<Connection>>` is explicitly disallowed for write-path orchestration logic.

## Why This Backlog Decision
- Markdown as source-of-truth is vulnerable to merge/edit collisions and ambiguous task identity.
- SQLite gives atomic lease/ack transitions, deterministic task ordering, and easy durability for worker recovery.
- Keeping a generated markdown view preserves legibility for humans/agents without reintroducing coordination bugs.
- External research note: Beads emphasizes issue-state ergonomics; current Beads README describes a Dolt-backed tracker model, which reinforces using structured state over free-form markdown as the canonical task store for concurrency-sensitive workflows.

## What We Are Not Doing
- No partial compatibility runtime where TypeScript remains orchestrator-of-record.
- No introduction of additional PR providers beyond `gh`.
- No expansion into multi-repo orchestration.
- No broad redesign of product-domain code unrelated to orchestrator reliability.
- No prompt re-engineering during cutover; V1 prompt content is ported from existing `scripts/ralph` prompts and tuned later.

## Implementation Approach
Implement a dedicated `gardener` Rust crate under `tools/gardener`, with strict dependency inversion for process execution, filesystem, time, and terminal I/O so all branches are testable and deterministic.

## Startup Sequence
1. Parse CLI and load config.
2. Resolve effective working directory from `--working-dir`/`scope.working_dir` with repo-root fallback policy.
3. Initialize logging and run ID.
4. Open SQLite, run migrations, and execute crash recovery on stale leases.
5. Reconcile worktrees:
   - parse `git worktree list --porcelain`
   - recover resumable branches
   - remove orphaned branches with no open PR.
6. Validate quality grades:
   - if missing or stale, run Gardener-integrated quality-grade refresh flow.
   - during cutover only: fallback to legacy repo-defined refresh command if integrated path is unavailable, and emit migration `P0`.
   - if still missing/failing, enqueue `P0` infra task with evidence.
7. If `startup.validate_on_boot=true`, run configured startup validation command (`startup.validation_command`) in the effective working directory and enqueue `P0` if red.
8. Import open PRs and merge-conflict signals from `gh` into backlog.
9. If backlog is empty, trigger agent-driven backlog seeding using:
   - quality-grade evidence
   - repository conventions/architecture context
   - codex-agent legibility/reliability principles.
   Persist seed prompt/context/task output artifacts for audit.
   V1 seeding runner uses direct `codex exec` CLI path from startup module (no dependency on Phase 6 adapter trait).
10. Start TUI (or non-TTY text mode).
11. Start worker pool and scheduler.
12. Run event-driven startup dispatch gate:
    - resume assigned or in-progress tasks first
    - otherwise claim ready queue
    - otherwise process pending external signals (PR changes, backlog seeding deltas)
    - otherwise enter idle watch loop with escalation thresholds.
13. Stop only when explicit termination condition is met.

`--prune-only` behavior by phase maturity:
- Phase 1 contract: config + CLI parse + working-dir resolution + capability probe + clean exit `0` (no git/backlog side effects).
- Phase 3+ contract: execute startup reconciliation prune logic and exit.

Startup reconciliation rules (normative):
- worktree exists + task status `complete`:
  - if no open PR and no unmerged commits against base, prune worktree/branch.
  - otherwise create `P0` reconciliation task with evidence.
- worktree missing + task status `in_progress`/`leased`:
  - release lease, increment attempt, set `ready`.
- PR ingest upsert:
  - key: `related_pr`,
  - open PR without task -> create task (`pr_collision` or mapped kind),
  - open PR with existing task -> refresh `last_updated` and metadata,
  - merged/closed PR -> mark related non-terminal PR-collision tasks complete or reclassify.
- Startup validation gate failure condition:
  - any required startup step returns typed error and cannot self-heal (quality refresh, schema migration, adapter probe, PR ingest).

Quality-grade staleness rule (normative):
- Report is stale when any condition is true:
  - `Last updated` date is older than `quality_report.stale_after_days`,
  - `stale_if_head_commit_differs=true` and recorded `repo_head_sha` metadata differs from current HEAD,
  - required domain set changed since last report generation.
- Report metadata fields required in generated document footer:
  - `last_updated_utc`
  - `repo_head_sha`
  - `working_dir`
  - `generator_version`

## Completion Observers and Reconciliation (Redundant by Design)
- Observer A: worker completion path applies terminal task transition and teardown.
- Observer B: scheduler reconciliation sweep repairs missed terminal transitions.
- Observer C: startup reconciliation repairs orphaned `in_progress`/leased state.
- All observers call the same idempotent reconciliation routine and emit typed evidence events.

## Worktree Lifecycle Invariants
- Create/resume on claim:
  - if branch worktree exists and has meaningful commits, resume.
  - if it exists without meaningful commits, remove and recreate.
- On complete (done-means-gone):
  - verify merge outcome
  - terminate active agent session and sandbox resources
  - remove worktree path
  - delete local branch
  - clear worker session bindings
  - mark task `complete`.
- On failure:
  - preserve evidence artifacts
  - release lease and requeue/escalate by policy.
- On startup prune:
  - any gardener-owned worktree not attached to active lease and without open PR is deleted.
- All operations are idempotent and return typed errors for deterministic escalation.

Meaningful-commit definition (normative):
- A worktree has meaningful commits when:
  - `git rev-list --count <base>..<branch> >= 1`, and
  - diff vs base contains at least one non-generated file change under `scope.working_dir`.
- Auto-generated snapshot/log-only commits are not meaningful for resume decisions.

## Worker Identity, Session, and Sandbox Boundaries
- `worker_id`: stable slot identity used for scheduling/fairness.
- `session_id`: per-task-attempt identity; regenerated on each retry/resume.
- `sandbox_id`: execution environment identity for adapter process/worktree binding.
- Invariants:
  - session state must never leak across task attempts.
  - sandbox teardown is required before session close.
  - all lifecycle/escalation events must include `worker_id` and `session_id`.
  - on process restart, `worker_id` remains slot-stable (`0..parallelism-1`), `session_id` is always regenerated, and resumed tasks include `resume_of_session_id` linkage in events.

## Worker FSM (Normative)

```text
IDLE -> UNDERSTAND -> (PLANNING?) -> DOING -> GITTING -> REVIEWING
REVIEWING(fail, cycle<3) -> DOING
REVIEWING(pass) -> MERGING -> COMPLETE -> IDLE
REVIEWING(fail, cycle>=3) -> FAILED/PARKED -> IDLE
Any state fatal error -> FAILED -> IDLE
```

Transition constraints:
- `task_type=task|chore|infra` skips `PLANNING`.
- `task_type=feature|bugfix|refactor` requires `PLANNING`.
- `DOING` max turns: `100`.
- Review loop max cycles: `3`.
- `MERGING` must verify merged state via `gh pr view` before marking `COMPLETE`.
- `COMPLETE` transition must execute done-means-gone teardown and emit completion evidence.

Git responsibility boundary (normative):
- Agent-owned git/gh operations (primary path):
  - branch creation/switching
  - commits and push
  - PR creation/update and conflict resolution attempts
- Orchestrator-owned git/gh operations (bounded, deterministic):
  - pre/postcondition checks (`gh pr view`, `git merge-base`, branch/worktree presence)
  - startup reconciliation/prune safety actions
  - terminal cleanup and escalation when invariants fail
- Non-goal:
  - orchestrator does not implement general-purpose autonomous git surgery logic in FSM states.

Merge verification contract (normative):
1. `gh pr view <n> --json state,mergedAt,mergeCommit` returns merged state.
2. `git merge-base --is-ancestor <merge_commit> <target_branch_head>` must pass.
3. configured post-merge validation command runs and exits `0` (default: `npm run validate`, overridable).
4. only then transition to `COMPLETE`.

Post-merge validation failure policy (normative):
- If merge is confirmed but post-merge validation fails:
  1. mark task terminal state as `complete_with_regression` (not plain `complete`),
  2. emit `P0` follow-up remediation task with failure evidence + merge SHA,
  3. do not attempt automatic unmerge/revert in V1.

## Internal Event Protocol (Typed, Normative)

```rust
pub enum GardenerLifecycleEvent {
    TaskLeased { task_id: String, worker_id: u32, session_id: String },
    TaskStarted { task_id: String, worker_id: u32, session_id: String, state: WorkerState },
    ReviewFailed { task_id: String, worker_id: u32, session_id: String, cycle: u8 },
    MergeReady { task_id: String, worker_id: u32, session_id: String, pr_number: u64 },
    TaskCompleted { task_id: String, worker_id: u32, session_id: String, merge_sha: String },
    TaskEscalated { task_id: String, severity: EscalationSeverity, reason: String },
    WorkerStalled { worker_id: u32, session_id: String, age_seconds: u64 },
    WorkerRecovered { worker_id: u32, session_id: String },
}
```

Protocol requirements:
- No stringly-typed lifecycle or escalation events in orchestrator internals.
- Protocol payloads are versioned and serialized deterministically.
- Unknown or malformed protocol payloads are rejected with typed errors and escalation path.

## Agent Adapter Contract (Normative)

```rust
#[async_trait]
pub trait AgentAdapter: Send + Sync {
    async fn run(
        &self,
        cfg: AgentRunConfig,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<AgentRunResult, GardenerError>;
}

pub struct AgentRunConfig {
    pub state: WorkerState,
    pub prompt: String,
    pub cwd: std::path::PathBuf,
    pub model: String,
    pub max_turns: u32,
    pub schema: Option<serde_json::Value>,
    pub execution_profile: AdapterExecutionProfile,
    pub worker_id: u32,
    pub session_id: String,
    pub sandbox_id: String,
}

pub enum AgentEvent {
    ToolCall { tool: String, summary: String },
    ToolResult { tool: String, output_preview: String },
    TurnComplete { turn: u32, tokens: u64 },
    OutputText { text: String },
    Error { message: String },
}
```

Contract requirements:
- Claude and Codex adapters pass the same contract suite for success/failure/timeout/cancel/hang.
- All streamed events normalize to shared `AgentEvent` variants.
- Structured outputs for each state deserialize to typed structs.
- Adapter events are translated into typed internal protocol events before scheduler consumption.

## Structured Output Extraction (Normative)
- All state prompts require exactly one final JSON envelope:
  - start marker: `<<GARDENER_JSON_START>>`
  - end marker: `<<GARDENER_JSON_END>>`
- Envelope payload schema:
  - `{ "schema_version": 1, "state": "<STATE>", "payload": { ...typed fields... } }`
- Parser algorithm:
  1. for Claude (stream-json): extract `result` string from the terminal `{"type":"result","subtype":"success","result":"..."}` NDJSON message; for Codex: use output file or terminal JSONL event text.
  2. locate the last complete `<<GARDENER_JSON_START>>` / `<<GARDENER_JSON_END>>` marker pair within that string,
  3. parse enclosed JSON strictly,
  4. validate `state` matches current FSM state,
  5. deserialize `payload` into state-specific struct.
- Failure behavior:
  - missing terminal result message, missing markers, invalid JSON, or schema mismatch -> typed `StructuredOutputError` and retry/escalation path.
- This contract is adapter-format-aware: marker scanning operates on the extracted result string, not raw stdout bytes.

## Prompt Packet Contract (Normative, Non-Black-Box)
- Prompt construction is deterministic and state-scoped; templates are versioned in `prompt_registry`.
- Every prompt packet contains:
  - `task_packet`: objective, in-scope boundaries, out-of-scope boundaries, acceptance checks, size budget, retry history.
  - `repo_context`: relevant conventions, architecture summaries, and repository policy snippets.
  - `evidence_context`: ranked file/symbol snippets with source path + line anchors and reason-for-inclusion.
  - `execution_context`: validation commands, merge mode, sandbox/tool constraints, and forbidden actions.
  - `knowledge_context`: active success/failure patterns selected from `knowledge_entries` by scope relevance.
- Context assembly algorithm:
  1. Build candidate context from task scope and discovered domain/component map.
  2. Rank candidates by relevance (direct path match > symbol dependency > domain adjacency > global policy).
  3. Apply deterministic token budget trimming with mandatory sections preserved.
  4. Emit `context_manifest` (ordered list of included sources with hashes) for audit/replay.
- Determinism details:
  - token budget source is `prompts.token_budget.<state>`.
  - relevance score formula is deterministic:
    - direct task path hit: +100
    - same domain/component: +40
    - symbol reference hit: +25
    - architecture/convention doc: +15
    - global policy doc: +10
  - ranking sort key is `(relevance_score DESC, path ASC, start_line ASC)`.
  - content hash algorithm is `sha256(path + \"\\n\" + start_line + \"\\n\" + end_line + \"\\n\" + snippet_bytes)`.
  - `context_manifest_hash = sha256(concatenated_manifest_entries_in_order)`.
- Token trimming algorithm (normative):
  - token estimator: deterministic `estimated_tokens = ceil(utf8_bytes / 4)`.
  - mandatory section order: `task_packet`, `execution_context`, `repo_context`.
  - optional section order: `evidence_context`, `knowledge_context`.
  - if optional sections overflow budget, trim from lowest-ranked entries first.
  - if mandatory sections alone overflow, keep full `task_packet` and truncate `repo_context` last.
  - hard floor: packet must include non-empty `task_packet` + `execution_context`; otherwise fail with typed prompt-build error.
- State-specific required input shaping:
  - `UNDERSTAND`: task packet + minimal repo context + decomposition hints.
  - `PLANNING`: add architecture context and prior failure patterns for similar scope.
  - `DOING`: include narrowed code context and explicit validation matrix for the task type.
  - `GITTING`: include commit/PR requirements and changed-file summary.
  - `REVIEWING`: include diff, validation outputs, and failure taxonomy checklist.
  - `MERGING`: include merge policy, conflict strategy, and post-merge verification steps.
- Prompt observability:
  - each run logs `prompt_version`, `context_manifest_hash`, and `knowledge_refs`.
  - deterministic replay with same snapshot must produce byte-equivalent prompt packets.

## Learning Loop (Outcomes -> Better Future Prompts)
- Post-merge analyzer (`postmerge_analysis.rs`):
  - extracts success patterns from completed tasks (validation flow used, code-change shape, review outcome).
  - writes/updates scoped `knowledge_entries` with confidence and evidence.
- Failure postmortem analyzer (`postmortem.rs`):
  - classifies failure root cause (context miss, wrong scope, validation miss, merge friction, tool/runtime issue).
  - records anti-pattern/guardrail entries with reproduction evidence.
- Knowledge curation rules:
  - only evidence-backed entries are active;
  - stale/low-confidence entries decay or deactivate automatically;
  - contradictory entries are flagged and quarantined until reviewed.
- Prompt integration:
  - prompt assembly pulls top relevant active entries (success + guardrail) by task scope.
  - applied knowledge refs are logged; later outcomes update entry confidence.
- Non-goal guardrail:
  - no opaque auto-learning from raw model text without evidence anchors.

Knowledge selection ranking (normative):
- Candidate match scores:
  - exact `path:<prefix>` hit: +80
  - exact `component:<id>` hit: +70
  - exact `domain:<name>` hit: +60
  - global entry: +20
- Final ranking score: `match_score + round(confidence * 20) + min(applies_count, 10)`.
- Selection cap per prompt packet:
  - max 8 entries total
  - max 3 guardrails
  - max 2 entries from any single scope key.

## JSONL Event Schema (Canonical)

```json
{"ts":"2026-02-27T14:31:42Z","run_id":"20260227T143000Z","event":"task_claimed","worker":0,"session_id":"s-001","task_id":"...","priority":"P0"}
{"ts":"2026-02-27T14:31:43Z","run_id":"20260227T143000Z","event":"state_transition","worker":0,"session_id":"s-001","from":"IDLE","to":"UNDERSTAND","task_id":"..."}
{"ts":"2026-02-27T14:31:50Z","run_id":"20260227T143000Z","event":"tool_call","worker":0,"session_id":"s-001","state":"DOING","tool":"Bash","summary":"<validation.command>","prompt_version":"doing.v3","context_manifest_hash":"abc...","knowledge_refs":["k-17","k-22"]}
{"ts":"2026-02-27T14:32:20Z","run_id":"20260227T143000Z","event":"pr_created","worker":0,"session_id":"s-001","task_id":"...","pr_number":88,"branch":"change-042"}
{"ts":"2026-02-27T14:33:35Z","run_id":"20260227T143000Z","event":"worker_stalled","worker":2,"session_id":"s-009","age_seconds":420}
{"ts":"2026-02-27T14:34:11Z","run_id":"20260227T143000Z","event":"task_complete","worker":0,"session_id":"s-001","task_id":"...","merge_sha":"abc123"}
```

Required event families:
- lifecycle: `task_claimed`, `task_requeued`, `task_failed`, `task_complete`
- state: `state_transition`, `review_cycle`
- tooling: `tool_call`, `tool_result`, `agent_error`
- git/pr: `worktree_created`, `pr_created`, `merge_attempt`, `merge_success`, `merge_conflict`
- startup: `startup_summary`, `quality_grades_bootstrap`, `worktree_prune`
- watchdog/problems: `worker_stalled`, `worker_recovered`, `lease_force_released`, `session_teardown_complete`
- prompt: `prompt_packet_built`, `prompt_packet_replayed`
- learning: `postmerge_pattern_recorded`, `failure_postmortem_recorded`, `knowledge_entry_applied`, `knowledge_entry_deactivated`

## Runtime Termination Conditions (Canonical)
- `--task "..."`: exit after that single task reaches terminal state.
- `--target N`: exit after `N` tasks complete in current run.
- `--prune-only`: perform startup reconciliation/prune and exit.
- `--backlog-only`: perform startup + backlog seeding + snapshot export and exit.
- `--quality-grades-only`: perform domain discovery + quality grading + doc write and exit.
- Normal autonomous exit: backlog empty and all workers idle for configured quiet window.
- Safety shutdown: max consecutive failures reached.

## Request-Order Fairness Data Model (Normative)
- Scheduler maintains an in-memory FIFO queue of worker pull requests:
  - `VecDeque<WorkRequest { request_id, worker_id, requested_at, responder }>`
- Dispatch loop:
  1. pop oldest `WorkRequest`,
  2. attempt atomic DB claim for highest-priority ready task,
  3. if task exists, assign to that requester,
  4. if no task exists, park request until signal tick.
- Fairness guarantee applies among workers that are simultaneously waiting for work.
- Metric `scheduler.wait_queue_depth` and trace `work_request_served` are required to verify ordering in tests.

Scheduler/worker interface contract (normative):
- Scheduler depends on a trait, not concrete FSM worker type:
  - `trait WorkerExecutor { fn execute(task_id, mode) -> TaskExecutionResult; }`
- Supported execution modes:
  - `normal` (FSM-driven)
  - `stub_complete` (Phase 4 validation mode).
- This seam is mandatory to keep Phase 4 independent from Phase 5.

## Projection Drift Detection and Repair (Normative)
- Steady-state detector runs every `scheduler.reconcile_interval_seconds`:
  - compare projection `last_event_id` against max applicable event id per entity,
  - validate legal state transitions from event stream,
  - flag missing projection rows for active tasks/workers.
- Repair strategy:
  1. replay events from last known consistent checkpoint for affected entities,
  2. if checkpoint missing or replay mismatch persists, rebuild full projections from event `id=1`,
  3. emit `projection_repair_performed` event with scope and row counts.
- If full rebuild fails integrity checks, escalate `P0` and halt scheduling.

## Task Identity Hash Contract (Normative)
- `task_id` is `sha256(canonical_task_identity_json)`.
- Canonical identity fields:
  - `kind` (`quality_gap`, `merge_conflict`, `pr_collision`, `feature`, `bugfix`, `maintenance`, `infra`)
  - normalized `title` (trimmed, lowercase, single-space)
  - normalized `scope_key` (domain/component/path bucket)
  - `related_pr` (or `null`)
  - normalized `related_branch` (or `null`)
- `details` and `source` are excluded from identity so equivalent tasks from different observers dedupe.
- Collision policy:
  - if same `task_id` reappears with higher priority, existing row is upgraded and `last_updated` touched.
  - if same `task_id` reappears with lower priority, priority is preserved.

`scope_key` derivation (normative):
- Order of derivation:
  1. explicit `scope_key` from source payload (if present and valid),
  2. mapped domain from referenced file paths,
  3. component identifier from architecture mapping,
  4. fallback `path:<top-level-dir>` within `scope.working_dir`,
  5. fallback `global`.
- Format:
  - `domain:<slug>` | `component:<id>` | `path:<prefix>` | `global`.

## Autonomous Execution Protocol
- Execute phases strictly in numeric order (`Phase 1` through `Phase 10`).
- A phase is complete only when all of its `Changes Required` are implemented and all `Success Criteria` are passing.
- Phase Validation Gate is mandatory at the end of every phase:
  1. run full unit test suite for Gardener (`cargo test -p gardener --all-targets`),
  2. run coverage and verify current `tools/gardener/src/**` code is at 100.00% line coverage (`cargo llvm-cov -p gardener --all-targets --summary-only`),
  3. run the phase-specific end-to-end binary scenario and confirm expected behavior with exit code `0`.
- On phase completion, continue immediately to the next phase with no manual approval step.
- If a phase fails validation, fix within the same phase until its success criteria pass; do not skip forward.
- No deferred validation: failures discovered in phase `N+1` that were introduced in phase `N` require returning to phase `N` and re-running its gate.
- Final autonomy definition of done:
  - configured repository validation gate passes (Brad OS default: `npm run validate`).
  - Gardener runtime paths are active and legacy `scripts/ralph` runtime paths are removed.
  - `npm run test:gardener:coverage` reports 100% line coverage for `tools/gardener/src/**`.
