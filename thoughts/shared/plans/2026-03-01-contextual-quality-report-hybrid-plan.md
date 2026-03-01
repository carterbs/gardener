# 2026-03-01 Contextual Quality Report Hybrid Plan (Lean Revision)

## Overview
Build contextual quality reports that work across repositories while staying deterministic. Ship this incrementally by extending the current Rust pipeline, not replacing it.

## Why This Revision
Claude's critique is directionally right on scope risk: the prior version read like a subsystem rewrite. This revision keeps your intent but narrows v1 to changes that can land quickly and produce visible output improvements.

## Current Baseline (Ground Truth)
1. Quality generation today is deterministic and startup-driven.
- Pipeline: `discover_domains -> collect_evidence -> score_domains` in [quality_grades.rs](../../../../tools/gardener/src/quality_grades.rs#L25), [quality_evidence.rs](../../../../tools/gardener/src/quality_evidence.rs#L15), [quality_scoring.rs](../../../../tools/gardener/src/quality_scoring.rs#L12).
- Signals are Rust-centric textual heuristics (`#[test]`, `mod tests`, `append_run_log(`) in [quality_evidence.rs](../../../../tools/gardener/src/quality_evidence.rs#L233).
2. Scope primitives exist (`repo_root`, `working_dir`) but no quality-specific scoring mode abstraction.
- [types.rs](../../../../tools/gardener/src/types.rs#L178), [config.rs](../../../../tools/gardener/src/config.rs#L530).
3. Startup report trigger exists and should remain the default execution path.
- [startup.rs](../../../../tools/gardener/src/startup.rs#L98).

## Product Decisions (Closed)
1. Single report artifact remains the product surface.
- Report has two sections: `Deterministic Findings` and `Contextual Insights`.
2. Deterministic scoring remains source of truth for grades.
3. LLM output is optional, explicitly labeled, and non-scoring in v1.
4. Do not execute repo-discovered lint/typecheck/test commands in v1.
- v1 records command/config presence only.
5. CI signal ingestion from external APIs is out of MVP scope.
- MVP uses repository-local workflow/config evidence only.

## Requirements (MVP)
1. Deliver a Rust-first MVP that materially improves report quality for this repository.
2. Keep scoring deterministic and reproducible.
3. Support monorepo subdirectory operation without score inflation.
4. Preserve startup generation flow and existing report consumers.
5. Keep implementation incremental on existing files unless extension requires new module.

## Non-Goals (MVP)
1. No GitHub Checks API ingestion.
2. No auto-running arbitrary repository commands.
3. No TS/JS/polyglot scoring in MVP (Rust-only scoring path in MVP).
4. No rubric governance process overhead beyond lightweight version tag in code.
5. No KPI dashboard system.

## Architecture (Incremental)

### 1) Extend Existing Evidence Model (No Ledger Rewrite)
Add minimal optional fields to current evidence/domain structures instead of introducing a new event ledger abstraction in MVP.

MVP additions:
- `language_counts` (Rust only in MVP; extensible for later packs)
- `scope_origin` (`subtree`, `repo_global`)

This keeps compatibility with current scoring code paths while enabling broader signals.

### 2) Scoped Scoring Mode in Quality Config (Not RuntimeScope)
Add `quality_report.scoring_scope` in config (`repo | subtree`), defaulting to:
- `repo` when `working_dir == repo_root`
- `subtree` when `working_dir != repo_root`

Rationale: scope mode is a quality-report behavior choice, not a core runtime scope primitive.
Mode semantics:
- `repo`: score from full repository evidence.
- `subtree`: score only from files under `working_dir`.
- `hybrid` is deferred to post-MVP once subtree attribution correctness is proven.

### 3) Deterministic Monorepo Attribution Rules (MVP)
For scoped runs:
1. Files under `working_dir` are `subtree` evidence.
2. Root policy files (`.github/workflows/**`, root lint/type/test config, shared toolchain manifests) are `repo_global`.
3. `repo_global` evidence is informational unless directly attributable to subtree by path filter.
4. Unknown attribution has zero numeric impact on scoped score.

### 3b) Rust Discovery Contract (MVP Concrete Spec)
Source discovery:
- Scan Rust source roots used by this repo (`src/**`, relevant crate/workspace Rust source directories).
- Include extension: `.rs`.
Test discovery:
- Content signals: `#[cfg(test)]`, `#[test]`, `mod tests`, `mod test`.
- Integration tests: Rust integration test file patterns in test directories already used by current pipeline.
Domain mapping:
- Reuse one shared domain-matching function across discovery and evidence collection.
- Domain assignment uses normalized relative path and single mapping implementation.

### 4) Contextual Insights Section (LLM Optional, Non-Scoring)
Behind feature flag (default off in MVP):
- LLM may summarize unknowns and suggest follow-up checks.
- Schema-validated JSON only.
- Invalid output discarded.
- Never changes numeric grade in MVP.

## Implementation Phases

### Phase 1: Rust MVP + Scope Foundation (MVP)
Goal: deliver visible report improvement quickly with low risk.

Changes:
1. Fix scoped-read correctness precondition:
- fix path normalization and reads as one change set:
  - source path normalization must avoid duplicate `src/src/...` prefixes
  - all collected evidence paths are stored as repo-root-relative
  - all file-content checks read from `repo_root.join(relative_path)` (never implicit process CWD)
2. Unify duplicated domain matching logic into a single helper used by discovery + evidence stages.
3. Strengthen Rust evidence collection accuracy using shared domain matching + scoped-safe path resolution.
4. Add `quality_report.scoring_scope` parsing and defaults.
5. Add scope attribution tags (`subtree`/`repo_global`) to collected evidence and render them in report evidence tables.
6. Keep existing grade formula shape with explicit bounded additions:
- existing base formula remains unchanged.
- scoped run guardrail: unattributed `repo_global` evidence contributes `0` to score.
- scoped `subtree` guardrail: if attributed `source_count == 0` for a domain, score is `0` in subtree mode (no default C fallback).

Success criteria:
1. Any grade changes from path-fix are explainable and expected (for example, previously-undetected inline tests now counted); no unexplained regressions.
2. Pre-fix vs post-fix per-domain score deltas are captured and explained; path-fix-only changes should not reduce a domain's score.
3. Rust scoped runs have correct file attribution and deterministic output across reruns.
4. Scoped runs do not improve score from unattributed root-level signals.
5. Existing `docs/quality-grades.md` format remains parse-compatible.
6. Shared domain-matching helper is the only path-to-domain mapping implementation in this pipeline.
7. Infrastructure remains residual (unmatched set) and does not duplicate explicit-domain matches.

### Phase 2: Deterministic Fairness Hardening (Still MVP)
Goal: make scoped/monorepo behavior robust and testable.

Changes:
1. Add explicit numeric caps for scoped penalties from repo-global signals (small bounded cap).
2. Add fixture tests for unrelated-root-failure vs scoped-health scenarios.
3. Harden path-based attribution for repo-local workflow/config evidence used in subtree mode.
4. Add characterization test before matcher unification:
- run current discovery matcher and evidence matcher over repo `src/**`
- diff assignments per file/domain
- document intentional behavior choice and lock with fixture.

Success criteria:
1. Unrelated monorepo root failures do not materially degrade scoped score.
2. Attributable scoped policy failures do degrade score predictably.
3. Re-running on same commit/config yields same score.

### Phase 3: Optional Contextual Insights (Post-MVP)
Goal: add LLM value safely after deterministic base is trusted.

Changes:
1. Add optional LLM pass for contextual narrative only.
2. Add strict schema validation + citation requirements to deterministic evidence keys.
3. Render separate `Contextual Insights` section with confidence labels.

Success criteria:
1. Numeric grades identical with LLM on/off.
2. Invalid model payloads are dropped with no report corruption.
3. Users can disable feature with one config flag.

### Phase 4: Polyglot Expansion (Post-MVP)
Goal: extend proven Rust MVP patterns to additional ecosystems.

Changes:
1. Add TS/JS detection/scoring pack first.
2. Add Python/Go only after TS/JS pass criteria are met.
3. Reuse shared domain matcher and scoped attribution semantics from MVP.
4. Introduce `hybrid` scoring mode only after subtree-mode attribution fixtures prove stable.

Success criteria:
1. Added language packs do not regress Rust scoring determinism.
2. Cross-language repos show meaningful non-zero evidence in newly supported packs.
3. Rollout remains opt-in per repo until fixture suite is stable.

## Testing Strategy
1. Unit:
- scoring_scope defaults
- attribution classification
- deterministic score invariants
- presence-signal parsing
 - path normalization invariants:
   - source files stored repo-root-relative (no `src/src/...`)
   - integration test paths stored repo-root-relative (for example `tests/...`, never absolute)
2. Integration:
- Rust-only fixture
- monorepo scoped fixture with unrelated root workflows
 - path-fix delta audit fixture:
   - capture pre-fix and post-fix scores
   - verify deltas are explainable and non-negative for path-fix-only effects
3. Compatibility:
- snapshot test to ensure existing report sections/tables remain parse-compatible.
4. Safety:
- verify no command execution occurs in MVP path.

## Migration and Compatibility
1. Keep output path and primary markdown structure stable (`docs/quality-grades.md`).
2. New sections/columns are additive.
3. If old parser assumptions break, fix parser compatibility as a bug before release.

## Pressure-Test vs 13 Questions (Re-assessed)
1. Gardener vs Ralph comparison and why Ralph feels better: addressed in baseline + design choices.
2. Avoid parity and support arbitrary repos: addressed via repo-agnostic incremental signals.
3. Flexibility + determinism: addressed via deterministic scoring and bounded optional insights.
4. LLM for gaps: addressed as optional, non-scoring, schema-bound phase.
5. Harness-engineering integration: addressed via fixture-first iterative hardening and measurable success gates.
6. Requirements first: addressed by closed MVP requirements/non-goals before phases.
7. Language support and phasing: addressed (Rust-only MVP; TS/JS then Python/Go post-MVP).
8. Language-agnostic lint/typecheck/test presence + pass/fail: MVP handles presence signals in Rust-first context; pass/fail execution deferred by design.
9. Tier merge decision: closed (single report artifact with two sections).
10. Polyglot repo support: addressed as explicit post-MVP Phase 4 rollout after Rust MVP stabilization.
11. Subdirectory support: addressed via `scoring_scope` defaults and scoped attribution.
12. Preserve repo-level signals without losing scope correctness: addressed via `repo_global` informational/default non-scoring semantics.
13. Prevent CI inflation in scoped mode: addressed via unknown=0 impact and bounded repo-global influence.

## Deferred Backlog (Explicit)
1. Command execution runner with sandbox/security model.
2. External CI checks ingestion (GitHub API).
3. Additional language packs beyond Rust (TS/JS first, then Python/Go).
4. Rubric version migration tooling.
5. KPI dashboarding/telemetry beyond local regression fixtures.
6. `tooling_presence` signals once Rust-specific file-glob semantics and consumers are fully specified.
7. `hybrid` scoring scope mode after subtree correctness is validated.

## Implementation Notes (Must-Touch Functions in MVP)
1. Path normalization + reads:
- `collect_source_files` in `quality_evidence.rs` (remove duplicate `src/` prefix behavior)
- `collect_integration_files` in `quality_evidence.rs` (normalize to repo-root-relative)
- `file_contains_tests` and `file_contains_instrumentation` in `quality_evidence.rs` (always read via `repo_root.join(relative_path)`).
2. Scoped zero-source scoring rule:
- `score_domain` in `quality_scoring.rs` must apply `source_count == 0 => 0` in subtree mode.

## Relevant Code References
- [quality_grades.rs](../../../../tools/gardener/src/quality_grades.rs#L25)
- [quality_scoring.rs](../../../../tools/gardener/src/quality_scoring.rs#L39)
- [quality_evidence.rs](../../../../tools/gardener/src/quality_evidence.rs#L233)
- [quality_domain_catalog.rs](../../../../tools/gardener/src/quality_domain_catalog.rs#L59)
- [startup.rs](../../../../tools/gardener/src/startup.rs#L98)
- [config.rs](../../../../tools/gardener/src/config.rs#L530)
- [types.rs](../../../../tools/gardener/src/types.rs#L178)

## External Reference
- OpenAI Harness Engineering: https://openai.com/index/harness-engineering/
