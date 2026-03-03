# Quality Grading System — Pre-Plan Requirements

## Architecture

The system is an **agent-driven assessment**. A coding agent receives a prompt, a set of tools, and a target repo path. It uses the tools to gather evidence, reasons about what it finds, and produces structured scores. A deterministic formula converts those scores into letter grades and renders the output document.

```
[deterministic tools] → [agent assessment + scoring] → [deterministic grade computation + rendering]
```

---

## P0 — Must Have

**P0-1: Agent-readiness is the primary grading axis.** The quality grade measures how well a repo supports autonomous agent work. This includes but is not limited to: test coverage, steering docs, feedback loop speed, mechanical guardrails, and convention clarity. A repo with 95% line coverage but no `AGENTS.md`, no linter, and 40-minute CI is not an A.

**P0-2: LLM-driven domain discovery.** The agent discovers domains by inspecting the repo — reading directory structure, READMEs, module boundaries, package manifests, and naming conventions. No hardcoded domain maps. The agent decides what constitutes a meaningful domain and names it. An optional `.gardener/domains.toml` can provide hints, but the agent must work without it.

**P0-3: Multi-language support as a first-class concern.** The agent must identify all languages present in the repo (and within each domain), assess test coverage per language, and flag languages with no test infrastructure. brad-os (TypeScript + Swift) is the first testbed. The tools must not assume a single language.

**P0-4: Deterministic evidence-gathering tools.** Provide the agent with tools it can invoke to collect hard data:
- **Tree walker** — list source files, test files, and their languages, grouped by directory
- **Test file detector** — identify test files by language convention patterns (configurable registry)
- **Assertion counter** — count test cases and assertion calls per test file
- **Coverage parser** — ingest existing coverage artifacts (Istanbul JSON, lcov, Cobertura XML, Tarpaulin JSON) if present; report "no coverage data" if absent
- **Untested file finder** — for each source file, report whether a corresponding test file exists (collocated or conventional)
- **TODO/FIXME scanner** — count debt markers per file
- **Doc scanner** — check for presence and staleness of AGENTS.md, CLAUDE.md, README, conventions docs, contributing guides
- **CI/lint detector** — check for CI config files, linter configs, pre-commit hooks, coverage thresholds

These tools produce structured JSON. They do not interpret — the agent interprets.

**P0-5: Agent produces structured scores.** After running tools and reasoning about the results, the agent outputs a structured JSON payload:
- **Per-domain scores** (0–100 scale) for: test coverage, test quality (assertion density), risk exposure (untested critical paths), and convention adherence
- **Repo-wide scores** (0–100 scale) for: agent steering, mechanical guardrails, local feedback loop, coverage infrastructure, documentation quality
- **Structural deficiencies** — a list of specific gaps that make the agent's job harder (e.g., "no coverage tooling configured," "no linter," "no AGENTS.md," "tests exist but no way to run them from CLI")

**P0-6: Deterministic grade computation from scores.** A pure function maps the agent's numeric scores to letter grades (A, B+, B, B-, C+, C, C-, D, F — 9-level scale). The formula is transparent and auditable. The agent doesn't pick the letter — it picks the numbers, and the formula picks the letter.

**P0-7: Structural deficiencies feed the backlog.** Every structural deficiency the agent identifies (no coverage tooling, missing steering docs, no linter, etc.) should be emitted as a candidate backlog task with a priority. The quality grade document is both a report card and a work intake source.

**P0-8: Zero-config operation.** The system must grade any repo given only a path. No setup, no config files, no prior knowledge. The agent figures it out. Configuration only refines.

---

## P1 — Important

**P1-1: Risk-weighted assessment via agent judgment.** The agent — not a regex — decides which untested files are high-risk. It reads the file, understands what it does (auth? payments? AI integration? data deletion?), and assigns risk accordingly. The tools surface the untested files; the agent assesses their risk.

**P1-2: TODO/FIXME density as a quality signal.** The scanner tool counts debt markers per domain. The agent uses density as a signal for stale or neglected code. High density in a domain should pull its grade down. The agent decides how much.

**P1-3: Coverage gap → backlog task generation.** When coverage tooling doesn't exist, the agent should emit a high-priority backlog task to add it (not just note its absence). When coverage exists but is low in a domain, emit tasks targeting the specific untested files, prioritized by the agent's risk assessment.

**P1-4: Integration/e2e test detection and bonus.** The tools detect integration and e2e tests. The agent should weight these more heavily than unit tests for domains where integration behavior matters (API handlers, data pipelines, etc.). Again — the agent decides the weighting, not a formula.

**P1-5: Freshness enforcement.** The grade document has a TTL (default 1 hour). Stale grades block seeding runs. The grading system can be re-invoked automatically on startup when stale.

**P1-6: Convention adherence assessment.** The agent reads whatever convention docs exist (AGENTS.md, docs/conventions/, .editorconfig, linter configs) and spot-checks source files for compliance. This is inherently non-deterministic — only an LLM can judge whether code follows documented conventions.

---

## P2 — Nice to Have

**P2-1: Instrumentation/observability signal.** The tools detect logging, tracing, and metrics calls. The agent assesses whether critical paths have adequate observability. Bonus for well-instrumented domains.

**P2-2: AI-enriched per-domain notes.** The agent writes a one-sentence narrative note per domain explaining the grade and the most impactful improvement. These notes go directly into the output document.

**P2-3: Trend tracking.** Store previous grade snapshots so the document can show directional movement (improving/declining/stable) per domain.

**P2-4: Cross-platform awareness.** For repos with multiple platform targets (ios/, android/, web/), report per-platform test health within each domain.

---

## Output Format

A self-contained Markdown document with:
1. **Repo summary** — languages detected, overall readiness grade, primary gap
2. **Agent readiness table** — repo-wide dimension scores and grades
3. **Domain coverage table** — per-domain scores, grades, key metrics
4. **Structural deficiencies** — ranked list of gaps with suggested backlog tasks
5. **Per-domain notes** — one-sentence agent-written assessments
6. Timestamp and TTL metadata

---

## Anti-Requirements

- **No hardcoded domain names.** Domains come from the agent's analysis of the repo.
- **No language-specific scoring logic in the grade formula.** Language awareness lives in the tools and the agent's reasoning, not in the grade computation.
- **No repo-specific knowledge baked into the system.** The prompt, tools, and formula must work identically for gardener, brad-os, or any arbitrary repo.

---

## Context

Requirements derived from comparing quality scoring in two systems:
- **Gardener** (`tools/gardener/src/quality_scoring.rs` and related) — deterministic filesystem heuristic scoring with auto-refresh
- **Ralph** (`~/Documents/Dev/brad-os/scripts/ralph/` + `scripts/update-quality-grades.ts`) — Istanbul coverage-based scoring with assertion density, risk penalties, and AI-enriched annotations

This pre-plan synthesizes the strengths of both and shifts the architecture toward agent-driven assessment.
