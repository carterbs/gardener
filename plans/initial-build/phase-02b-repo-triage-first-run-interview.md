## Phase 02b: Repo Triage + First-Run Interview
Context: [Vision](./00-gardener-vision.md) | [Shared Foundation](./01-shared-foundation.md)

**Prerequisite:** Phase 01 (runtime skeleton, DI traits, config, output_envelope) must be complete.
**Blocks:** Phase 03 (startup audits — quality grade generation requires the repo intelligence profile as input).
**Parallel with:** Phase 02 (central backlog + priority classifier) — these phases have no dependency on each other.

---

### Purpose

When Gardener encounters a repository for the first time, it has no knowledge of the codebase beyond what it can infer from the working directory. The first-run triage closes this gap in three steps: it detects which agent the user is working with and confirms the choice, runs an agent-driven discovery pass that assesses the repository's agent-readiness across five dimensions, and conducts a short interactive interview to surface context that automated discovery can't find. The result is a `RepoIntelligenceProfile` — a versioned, committed document that anchors the first quality grade and every backlog seed that follows.

The five dimensions Gardener assesses:
1. **Agent steering** — is there explicit, well-structured guidance for coding agents? (AGENTS.md quality, `.claude/` skills, `.codex/config.toml`, complement vs. conflict)
2. **Knowledge accessible** — can an agent find what it needs to know? (in-repo is best; external-but-accessible via configured tool is acceptable; referenced-but-unreachable counts as zero)
3. **Mechanical guardrails** — are rules enforced by tooling, not just described? (standard linters, custom architectural linters, pre-commit hooks)
4. **Local feedback loop** — can an agent make a change, run something locally, and trust the result? (test framework present, validation command exists, deterministic output)
5. **Coverage signal** — does the test suite actually catch regressions? (meaningful coverage floor, not just tests that exist)

These map to the Codex article's framework. "Managed backlog" is intentionally excluded — that's Gardener's job, not a pre-existing repo attribute.

---

### When Triage Runs

Triage runs (and requires an interactive terminal) when:
- Profile file at `triage.output_path` does not exist, OR
- `--retriage` CLI flag is passed, OR
- Profile `meta.head_sha` differs from `git rev-parse HEAD` by more than `triage.stale_after_commits` commits (default: `50`).

**Triage requires a human.** There is no headless triage mode. If Gardener determines triage is needed but no human is present, it hard-stops with an actionable error — it does not proceed with defaults or skip questions.

**Non-interactive detection** (checked before triage runs, in priority order):
1. `CLAUDECODE` env var is set → running inside Claude Code
2. `CODEX_THREAD_ID` env var is set → running inside Codex
3. `CI` env var is set → running in a CI pipeline
4. stdin is not a TTY → generic automation (pipes, scripts)

**Behavior when non-interactive and triage is needed:**
```
Error: Triage requires a human and cannot run non-interactively.

No repo intelligence profile was found at .gardener/repo-intelligence.toml.
Triage gathers context that Gardener cannot determine automatically.

To complete setup, run in a terminal:
  brad-gardener --triage-only

Then re-run your agent or pipeline.
```

**Behavior when non-interactive and profile exists:** proceed normally. Triage already happened; a coding agent can run everything else without restriction.

The `--retriage` flag is only valid in interactive mode. If `--retriage` is passed in a non-interactive environment, Gardener exits with an error: *"--retriage requires an interactive terminal."*

---

### Changes Required

**New Rust modules:**
- `tools/gardener/src/triage.rs` — top-level orchestrator: `run_triage()`, sequences agent_detection → discovery → interview → validation → persist.
- `tools/gardener/src/triage_agent_detection.rs` — two responsibilities: (1) non-interactive environment detection (`is_non_interactive()` checks `CLAUDECODE`, `CODEX_THREAD_ID`, `CI`, and TTY via Terminal DI trait); (2) coding agent detection for Q0 (`detect_agent()` scans file system for Claude/Codex config signals and returns `DetectedAgent`).
- `tools/gardener/src/triage_discovery.rs` — agent-driven discovery pass: builds prompt, invokes agent via ProcessRunner + output_envelope, returns `DiscoveryAssessment`.
- `tools/gardener/src/triage_interview.rs` — interactive interview: Q0 (agent confirmation) through Q7, assumption validation, correction loop.
- `tools/gardener/src/repo_intelligence.rs` — `RepoIntelligenceProfile` struct + TOML serde + `AgentReadiness` derived fields: `build_profile()`, `write_profile()`, `read_profile()`, `is_stale()`.

**Config changes** — `[agent]` table replaces per-task backend/model scattered across `[triage]`, `[seeding]`, and `[states.*]`. See shared foundation Config Schema.

**New CLI flags:**
- `--retriage` — force triage even if profile is fresh (interactive only; errors in non-interactive environments).
- `--triage-only` — run triage then exit.
- `--agent <claude|codex>` — override detected agent; skips Q0 confirmation.

**Removed flag:** `--headless` is not implemented. Triage requires a human; there is no mechanism to bypass it.

**New test fixtures:**
- `tools/gardener/tests/fixtures/configs/phase02b-triage.toml`
- `tools/gardener/tests/fixtures/repos/triage-fully-equipped/` — AGENTS.md (pointer-style) + `.claude/` + `.codex/config.toml` + architecture docs + custom linter + tests with coverage
- `tools/gardener/tests/fixtures/repos/triage-minimal/` — only README.md
- `tools/gardener/tests/fixtures/repos/triage-claude-only/` — `.claude/` + CLAUDE.md, no `.codex/`
- `tools/gardener/tests/fixtures/repos/triage-codex-only/` — `.codex/config.toml` + AGENTS.override.md, no `.claude/`
- `tools/gardener/tests/fixtures/repos/triage-no-agents/` — test/lint signals but no agent config at all
- `tools/gardener/tests/fixtures/triage/expected-profiles/*.toml` — expected profile output per fixture
- `tools/gardener/tests/fixtures/triage/mock-discovery-responses/*.json` — mock output_envelope responses (used in `execution.test_mode = true`)

Each triage fixture repo has its own `.git/` per the fixture git isolation contract.

---

### Step 1: Agent Detection

Before invoking any agent, Gardener determines which agent(s) the user is working with. This is purely deterministic — no subprocess invocation.

**Signals scanned (in `scope.working_dir` and `repo_root`):**

| Signal | Indicates |
|--------|-----------|
| `.claude/` directory present | Claude |
| `CLAUDE.md` present | Claude |
| `.claude/settings.json` or `.claude/mcp.json` | Claude (strong) |
| `.claude/skills/` or `.claude/commands/` | Claude (strong) |
| `.codex/config.toml` present | Codex |
| `AGENTS.override.md` present | Codex (Codex-specific file) |
| `AGENTS.<name>.md` (non-override named variant) | Codex |
| `AGENTS.md` present | Both (shared format) |
| Neither `.claude/` nor `.codex/` | Unknown |

**Confidence result:** `DetectedAgent::Claude`, `DetectedAgent::Codex`, `DetectedAgent::Both`, or `DetectedAgent::Unknown`.

**Note on subdirectory scope:** Both Claude and Codex walk from repo root → CWD and load config/instructions at each level. When Gardener is scoped to a subdirectory, agent signals at the repo root are still active and should be detected. Scan both `scope.working_dir` and `repo_root`; report root-level signals separately so the operator knows which signals are outside their direct control.

**Confirmation prompt (Q0 — shown before the main interview):**

```
━━━ Agent Detection ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  I found the following agent configuration in this repository:
    {detected signals, e.g. "✓ .claude/ directory  ✓ CLAUDE.md  ✓ .claude/skills/"}
    {root-level notes if scoped, e.g. "  (root) ✓ AGENTS.md — applies to all subdirectories"}

  It looks like you're using {detected_agent}.
  Should I use it for the discovery pass and as the default for all Gardener tasks?

    1. Yes — use {detected_agent} for everything  (recommended)
    2. Use Claude instead
    3. Use Codex instead
    4. Configure per-task (advanced — I'll ask about each task separately)
    t. Other:

  Choice: _
```

For `DetectedAgent::Both`: present both, ask which to use as the primary.
For `DetectedAgent::Unknown`: skip "It looks like you're using" and go straight to the choice.

**After confirmation:** write `agent.default = "<choice>"` to the Gardener config TOML (at `--config` path). All task backends and models will default to Gardener's recommended mapping for that agent unless explicitly overridden. If the user chose option 4, the interview will include per-task configuration questions.

---

### Step 2: Agent Discovery Pass

The discovery pass is entirely agent-driven. There is no deterministic pre-scan phase. The agent receives a prompt with the working directory and instructions; it uses its tools to read files and form quality judgments.

**Why agent-only:** A deterministic scan can detect file *existence* but not file *quality*. Whether AGENTS.md is a useful pointer-style map or a 2,000-line monolith that will be silently truncated by Codex's 32 KiB cap — that judgment requires reading the file. We get deterministic *scores* from deterministic *formulas* applied to the agent's assessments. The inputs are non-deterministic; the scoring is not.

**Invocation:** Uses the same direct-runner pattern as Phase 03's legacy seed runner — ProcessRunner invokes the configured agent binary in non-interactive mode, bounded by `triage.discovery_max_turns` (default: 12). In `execution.test_mode = true`, loads mock response from `tests/fixtures/triage/mock-discovery-responses/<fixture-slug>.json`.

**Scope note for subdirectories:** The agent is told its working directory AND the repo root. It should assess the working directory scope, but it naturally reads upward (both Claude and Codex do this). Root-level signals that help or hurt the subdirectory should be flagged as `scope_notes` in the output — they're informational, not scored against the subdirectory's operator.

**Discovery prompt:**

```
You are performing an agent-readiness assessment for Gardener.

WORKING DIRECTORY: {working_dir}
REPOSITORY ROOT: {repo_root}

{if working_dir != repo_root:}
Note: You are assessing a subdirectory of a larger repository. Assess the working
directory scope. Root-level files (AGENTS.md, linter configs, CI config) are visible
to coding agents but may be outside this operator's control — flag them in scope_notes
rather than scoring them against this scope.

Your task is to assess how ready this directory is for a coding agent to do meaningful,
reliable work across five dimensions. Read the relevant files and form genuine quality
judgments. Don't just report what exists — assess whether it's actually useful to an agent.

STEP 1 — Read the quality criteria:
Read docs/references/codex-agent-team-article.md (relative to repo root).
This is the framework you will apply. Pay particular attention to what the article says
about AGENTS.md quality, architecture enforcement, and knowledge as system of record.

Note: recent research confirms that bloated AGENTS.md files actively degrade agent
performance. Codex enforces a hard 32 KiB cap on combined AGENTS.md content and
silently truncates beyond it. Assess AGENTS.md length and structure accordingly.

STEP 2 — Assess the five dimensions:

1. AGENT STEERING
   Look for: AGENTS.md (at all directory levels up to repo root), AGENTS.override.md,
   AGENTS.<name>.md variants, CLAUDE.md, .claude/ directory (skills/, commands/,
   settings.json, mcp.json), .codex/config.toml (at all levels).
   Assess:
   - Do steering docs exist for the agent(s) in use?
   - Is AGENTS.md pointer-style (~100 lines, table of contents) or a monolith?
   - Do CLAUDE.md and AGENTS.md complement each other or overlap/conflict?
   - Are skills or sub-agent definitions present and well-structured?
   - What is the combined AGENTS.md line count across all levels? (Codex 32 KiB ≈ ~500-700 lines)
   Grade A: concise pointer-style AGENTS.md + relevant skills/config
   Grade F: no steering docs, or monolith that crowds out task context

2. KNOWLEDGE ACCESSIBLE
   Look for: architecture docs (ARCHITECTURE.md, docs/architecture/, docs/design/),
   convention docs (CONTRIBUTING.md, docs/guidelines/, .editorconfig),
   any docs/ directory structure, MCP config in .claude/mcp.json.
   Assess:
   - Is relevant knowledge committed to the repository?
   - Are there links to external docs? If so, is there a configured MCP or tool that
     gives an agent access? (If external docs exist but no access mechanism: flag as
     inaccessible — counts as zero, same as not existing)
   - Is in-repo knowledge structured and navigable, or a pile of stale files?
   Grade A: structured in-repo docs covering architecture + conventions, or accessible external
   Grade F: knowledge lives in external systems with no agent-accessible bridge

3. MECHANICAL GUARDRAILS
   Look for: .eslintrc*, eslint.config.*, clippy.toml, ruff.toml, .flake8, biome.json,
   golangci-lint.yml, tools/lint*/, tools/arch-lint*/, scripts/lint*/,
   .pre-commit-config.yaml, .husky/, .lefthook.yml.
   Assess:
   - Do standard linters exist?
   - Are there custom linters that enforce architectural rules? (these are multipliers)
   - Are linter error messages written to inject remediation context for agents?
   - Are linters enforced at commit time or only optionally?
   Grade A: custom architectural linters with agent-friendly error messages + enforced at commit
   Grade F: no linting at all

4. LOCAL FEEDBACK LOOP
   Look for: jest.config.*, vitest.config.*, pytest.ini, pyproject.toml [tool.pytest],
   Cargo.toml [dev-dependencies], *_test.go, package.json test/validate scripts,
   Makefile test targets.
   Focus on LOCAL experience — can an agent run a command and trust the result?
   Assess:
   - Is there a clear validation/test command an agent can run locally?
   - Does it complete with a clear pass/fail signal?
   - Is the test suite fast enough for an agent to run during iteration?
   Do NOT assess CI enforcement here — focus on what the agent can do locally.
   Grade A: fast deterministic local command, clear signal, reliable
   Grade F: no tests or no runnable validation command

5. COVERAGE SIGNAL
   Look for: coverage-summary.json, codecov.yml, coverageThreshold in jest config,
   cargo-llvm-cov config, .lcov files.
   Assess:
   - Does the test suite cover meaningful code paths, or is coverage nominal?
   - Is there a coverage floor that would actually catch a regression?
   - Would an agent breaking something be caught by the test suite?
   Grade A: meaningful coverage with an enforced floor
   Grade F: minimal coverage or coverage that doesn't catch real regressions

STEP 3 — Return your findings as a JSON envelope:
{"gardener_output": {
  "agent_steering":        {"grade": "A-F", "summary": "...", "issues": [], "strengths": [], "agents_md_line_count": N, "agents_md_style": "pointer|monolith|mixed|absent"},
  "knowledge_accessible":  {"grade": "A-F", "summary": "...", "issues": [], "strengths": [], "external_docs_found": false, "external_access_configured": false},
  "mechanical_guardrails": {"grade": "A-F", "summary": "...", "issues": [], "strengths": [], "has_custom_linters": false},
  "local_feedback_loop":   {"grade": "A-F", "summary": "...", "issues": [], "strengths": [], "validation_command_detected": "cmd or null"},
  "coverage_signal":       {"grade": "A-F", "summary": "...", "issues": [], "strengths": []},
  "overall_readiness_score": 0-100,
  "overall_readiness_grade": "A-F",
  "primary_gap": "dimension_slug",
  "notable_findings": "...",
  "scope_notes": "root-level signals that affect this scope but may be outside operator control"
}}
```

**Failure handling:** If agent invocation fails or output is invalid: log WARN, set `meta.discovery_used = false`, set all dimension grades to `"unknown"`, continue to interview. The interview will gather assessments manually in this case.

---

### Step 3: Interactive Interview

The interview runs after the discovery pass. Each question shows the agent's finding for that dimension and asks the operator to validate or correct it. Every question has a free-text option.

#### Q1: Agent Steering Validation
```
━━━ Agent Steering ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Agent assessment: {agent_steering.grade} — {agent_steering.summary}
  {agent_steering.issues formatted as bullet list if non-empty}

  Does this match your understanding?
    1. Yes, that's accurate
    2. The assessment missed something — enter details:
    3. My situation is different — enter description:

  Choice: _
```

#### Q2: Knowledge Accessibility — Other Surfaces
```
━━━ Knowledge Accessibility ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Agent assessment: {knowledge_accessible.grade} — {knowledge_accessible.summary}
  {issues if any}

  Are there documentation surfaces the agent didn't check?
  (e.g., Confluence, Notion, Google Docs, internal wikis)
    1. No — the in-repo docs are the full picture
    2. Yes, we have external docs — describe location and how agents access them:
       (If no agent-accessible bridge exists, Gardener will flag this as a gap)

  Choice: _
```

#### Q3: Mechanical Guardrails
```
━━━ Mechanical Guardrails ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Agent assessment: {mechanical_guardrails.grade} — {mechanical_guardrails.summary}
  {issues if any}

  Does this match your understanding? Any custom linters I missed?
    1. Yes, accurate
    2. There are additional guardrails I missed — enter paths/descriptions:

  Choice: _
```

#### Q4: Local Feedback Loop
```
━━━ Local Feedback Loop ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Agent assessment: {local_feedback_loop.grade} — {local_feedback_loop.summary}
  Detected validation command: {validation_command_detected or "none detected"}

  What command should Gardener use to validate the repository?
    1. {validation_command_detected}  (use detected command)
    2. npm run validate
    3. cargo test --all-targets
    4. make test
    t. Type the command:

  Choice: _
```

(This answer is authoritative — it overrides the agent's detection and writes `startup.validation_command` to config.)

#### Q5: Coverage Signal
```
━━━ Coverage Signal ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Agent assessment: {coverage_signal.grade} — {coverage_signal.summary}

  How would you characterize your test coverage?
    1. Strong — meaningful floor, regressions get caught
    2. Moderate — critical paths covered, gaps at edges
    3. Sparse — many modules untested; agents work without a net
    4. Unknown — no formal tracking
    t. Describe your situation:

  Choice: _
```

#### Q6: Anything Else?
```
━━━ Anything Else? ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Is there anything important about this repository that I should know
  before generating the first quality grade?
    1. No — the above captures it well
    t. Yes:

  Choice: _
```

---

### Step 4: Assumption Validation

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Here's what I've learned about this repository:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  Agent in use:          {detected_agent}
  Agent steering:        {grade} — {summary}
  Knowledge accessible:  {grade} — {summary}
  Mechanical guardrails: {grade} — {summary}
  Local feedback loop:   {grade} — {validation_command}
  Coverage signal:       {grade} — {summary}

  ──────────────────────────────────────────────────────────────────
  Agent readiness:       {readiness_score}/100  ({readiness_grade})
  Primary gap:           {primary_gap} — this is what Gardener will address first
  ──────────────────────────────────────────────────────────────────

Do these assumptions look right?
  1. Yes — write the profile and proceed
  2. I want to correct something
  3. Abort

Choice: _
```

Correction: user picks which question (Q1–Q6) to revisit. Cap at 3 correction rounds.

---

### Repo Intelligence Profile Schema (Normative)

Written to `triage.output_path` (default `.gardener/repo-intelligence.toml`).

```toml
[meta]
schema_version = 1
created_at = "2026-02-27T00:00:00Z"
head_sha = "abc123..."
working_dir = "/path/to/scope"
repo_root = "/path/to/repo"
discovery_used = true              # false if agent invocation failed during discovery pass

[detected_agent]
primary = "claude"                 # "claude" | "codex" | "unknown"
claude_signals = [".claude/", "CLAUDE.md"]
codex_signals = [".codex/config.toml", "AGENTS.override.md"]
agents_md_present = true
user_confirmed = true              # false in headless mode

[discovery]
# Raw agent assessment per dimension — verbatim from output_envelope
[discovery.agent_steering]
grade = "B"
summary = "AGENTS.md is pointer-style at 87 lines. CLAUDE.md adds Claude-specific workflow..."
issues = ["AGENTS.md and CLAUDE.md have overlapping test instructions"]
strengths = ["Pointer-style, well under 32 KiB Codex cap", "Skills directory present"]
agents_md_line_count = 87
agents_md_style = "pointer"

[discovery.knowledge_accessible]
grade = "C"
summary = "Architecture docs in docs/architecture/ but no convention docs..."
issues = ["No CONTRIBUTING.md or convention docs found"]
strengths = ["Architecture domain docs present and structured"]
external_docs_found = false
external_access_configured = false

[discovery.mechanical_guardrails]
grade = "A"
summary = "Custom arch-lint enforces domain layering..."
issues = []
strengths = ["Custom linter with agent-friendly error messages", "Pre-commit hook configured"]
has_custom_linters = true

[discovery.local_feedback_loop]
grade = "B"
summary = "npm run validate runs tests + lint in under 30 seconds..."
issues = []
strengths = ["Fast local validation command", "Clear pass/fail signal"]
validation_command_detected = "npm run validate"

[discovery.coverage_signal]
grade = "C"
summary = "Jest coverage present but no threshold configured..."
issues = ["No coverage threshold — passing suite does not guarantee floor"]
strengths = ["Coverage tooling present"]

[user_validated]
agent_steering_correction = ""           # free text if user corrected Q1
external_docs_surface = ""               # free text if user added external docs (Q2)
external_docs_accessible = false         # true if user confirmed access mechanism
guardrails_correction = ""               # free text if user corrected Q3
validation_command = "npm run validate"  # authoritative — from Q4
coverage_grade_override = ""            # if user overrode agent grade (Q5)
additional_context = ""                 # Q6
corrections_made = 0
validated_at = "2026-02-27T00:00:00Z"

[agent_readiness]
# Derived deterministically from discovery + user_validated.
# Dimension weights sum to 100.
agent_steering_score = 14              # weight 20: grade B → 14/20
knowledge_accessible_score = 9        # weight 20: grade C → 9/20
mechanical_guardrails_score = 18      # weight 20: grade A → 18/20 (custom linters bonus)
local_feedback_loop_score = 14        # weight 20: grade B → 14/20
coverage_signal_score = 9             # weight 20: grade C → 9/20

readiness_score = 64
readiness_grade = "D"
primary_gap = "knowledge_accessible"
```

**Scoring formula (deterministic):**
```
dimension_score(grade, weight) =
  A → weight * 0.90
  B → weight * 0.70
  C → weight * 0.45
  D → weight * 0.25
  F → 0
  unknown → weight * 0.10   (headless or discovery failed)

readiness_score = sum of all five dimension_scores, clamped 0–100
```

Each dimension weight is 20. `primary_gap` = dimension with lowest score; ties broken by dimension order above.

If `user_validated.external_docs_found = true` AND `user_validated.external_docs_accessible = false`: force `knowledge_accessible` dimension to grade `F` regardless of agent assessment. Inaccessible context counts as nothing.

---

### Integration with Phase 03

Phase 03 reads the profile before quality grade generation:
- `detected_agent.primary` → confirms which agent is configured for seeding
- `user_validated.validation_command` → overrides `startup.validation_command` if unset
- `agent_readiness.*` fields → injected into seeding agent prompt; seeding agent must address `primary_gap` dimension first
- Quality grade document emits `## Triage Baseline` section: profile path, readiness_score, readiness_grade, primary_gap

---

### Success Criteria

- Agent detection correctly classifies all 5 fixture repo permutations (fully-equipped, minimal, claude-only, codex-only, no-agents).
- Q0 confirmation renders all three signal types (Claude detected, Codex detected, Unknown) via FakeTerminal.
- `--agent <claude|codex>` flag skips Q0 and writes `agent.default` directly.
- Discovery prompt is built correctly: working_dir, repo_root, and scope note (when scoped) are all present.
- Discovery pass invokes agent via ProcessRunner; mock response parsed via output_envelope; all five `DiscoveryAssessment` dimension fields populated.
- Agent invocation failure sets `meta.discovery_used = false`; interview proceeds with all grades = "unknown"; scoring uses the `unknown` formula.
- Interview renders Q1–Q6 via Terminal; each question shows the agent's grade and summary for that dimension.
- Q4 answer is treated as authoritative for `validation_command` regardless of agent detection.
- External docs declared in Q2 with no access mechanism → `knowledge_accessible` forced to grade F; WARN event emitted.
- Assumption validation step shows all five dimensions + readiness_score; correction loop re-asks selected question; cap at 3 rounds.
- Profile TOML written to `triage.output_path` via FileSystem; all required sections present; round-trips without loss.
- Readiness score derivation is deterministic: same inputs always produce same scores.
- `primary_gap` is always the lowest-scoring dimension; ties broken by defined dimension order.
- Non-interactive + profile missing: hard stop with actionable error message; no profile written; exit code non-zero. Tested for each non-interactive signal: `CLAUDECODE` set, `CODEX_THREAD_ID` set, `CI` set, stdin not a TTY.
- Non-interactive + profile exists: proceeds normally without entering triage. No error, no warning.
- `--retriage` in non-interactive environment: exits with error "requires an interactive terminal."
- `--triage-only` runs full triage and exits.
- `--retriage` forces re-triage even when profile is fresh.
- Staleness: profile with diverged head_sha correctly identified as stale.
- Phase 03 consumes profile: `validation_command` injected, `primary_gap` present in seeding prompt, `## Triage Baseline` in quality grade output.

---

### Phase Validation Gate (Mandatory)

- Run: `cargo test -p gardener --all-targets`
- Run: `cargo llvm-cov -p gardener --all-targets --summary-only` (100.00% lines for `tools/gardener/src/**`)
- E2E smoke (interactive, FakeTerminal, test_mode=true):
  ```
  scripts/brad-gardener --triage-only \
    --config tools/gardener/tests/fixtures/configs/phase02b-triage.toml \
    --working-dir tools/gardener/tests/fixtures/repos/triage-fully-equipped
  ```
  Assert: profile written, `detected_agent.primary` correct, `readiness_score` derived, `discovery_used = true`.
- Non-interactive hard-stop smoke (CLAUDECODE, profile missing):
  ```
  CLAUDECODE="" scripts/brad-gardener \
    --config tools/gardener/tests/fixtures/configs/phase02b-triage.toml \
    --working-dir tools/gardener/tests/fixtures/repos/triage-minimal
  ```
  Assert: exits non-zero, stderr contains `brad-gardener --triage-only`, no profile written.
- Non-interactive hard-stop smoke (CODEX_THREAD_ID, profile missing): same assertion with `CODEX_THREAD_ID=test-id`.
- Non-interactive proceed smoke (profile pre-exists):
  pre-seed profile fixture → run with `CLAUDECODE=""` → assert exits zero, triage not entered.
- External docs red flag smoke: fixture with `external_docs_found = true` + `external_docs_accessible = false` in mock response → `knowledge_accessible` grade forced to F in profile.
- Agent flag smoke: `--agent codex` on claude-only fixture → `agent.default = "codex"` written to config, Q0 skipped.

---

### Autonomous Completion Rule

- Continue directly to the next phase only after all success criteria and this phase validation gate pass.
- Do not wait for manual approval checkpoints.
