## Phase 3: Startup Audits (Quality Grades, Worktrees, PRs, Backlog Seeding)
Context: [Vision](./00-gardener-vision.md) | [Shared Foundation](./01-shared-foundation.md)

**Prerequisite:** Phase 02b (Repo Triage + First-Run Interview) must complete before this phase. The repo intelligence profile (`triage.output_path`) is a required input to quality grade generation and backlog seeding.
### Changes Required
- Add startup audit module:
  - `tools/gardener/src/startup.rs`
  - `tools/gardener/src/quality_grades.rs`
  - `tools/gardener/src/quality_domain_catalog.rs`
  - `tools/gardener/src/quality_evidence.rs`
  - `tools/gardener/src/quality_scoring.rs`
  - `tools/gardener/src/worktree_audit.rs`
  - `tools/gardener/src/pr_audit.rs`
  - `tools/gardener/src/seeding.rs`
  - `tools/gardener/src/seed_runner.rs` (temporary direct CLI runner adapter used only until Phase 6).
- Startup sequence:
  1. Load repo intelligence profile from `triage.output_path`:
     - if profile missing: hard stop regardless of interactive/non-interactive mode.
       Error message: "No repo intelligence profile found. Run `brad-gardener --triage-only` in a terminal to complete setup."
       A coding agent receiving this error should surface it to a human rather than attempting to proceed.
     - if profile stale (HEAD diverged by > `triage.stale_after_commits`): emit non-blocking `WARN`, continue with existing profile.
     - if `user_validated.validation_command` is set and `startup.validation_command` is not: inject profile value into startup config.
  2. Verify configured quality-grade output document path (`quality_report.path`).
  3. If missing or stale, run Gardener-integrated quality-grade generation:
     - load profile `[codex_readiness]` scores for the `## Agent Readiness` section of the unified document.
     - discover repository domains via deterministic detector order and domain definition contract from shared foundation.
     - map code/test/docs artifacts into discovered domains within the effective working directory scope.
     - compute per-domain coverage scores + evidence using the domain scoring rubric from shared foundation:
       - `codex_readiness.has_coverage_gates = false` → coverage component starts at 0 for all domains (not inferred from absent data).
     - write unified quality-grade document deterministically following the readiness-first output contract from shared foundation:
       - headline readiness score and grade from profile,
       - `## Triage Baseline` section: profile path, `readiness_score`, `readiness_grade`, `primary_gap`,
       - `## Agent Readiness` section: per-dimension table populated from profile `[codex_readiness]`,
       - `## Coverage Detail` section: per-domain grade table + evidence sections for tested files, untested files, and prioritized debt.
     - apply staleness policy from config (`quality_report.stale_after_days`, `quality_report.stale_if_head_commit_differs`).
  4. During cutover only, fallback to legacy repo-defined refresh command if integrated generator is unavailable; emit `P0` migration task.
  5. If quality-grade generation still fails, enqueue/execute `P0` infra task with diagnostics.
  6. Reconcile hanging worktrees (stale leases, missing paths, merged branches, detached leftovers).
  7. Ingest open/unmerged PR signals (`gh pr list`/`gh pr view`).
  8. If `startup.validate_on_boot=true`, run configured startup validation command and enqueue `P0` recovery task when red.
  9. Seed backlog via dedicated seeding runner:
     - invoke direct `codex exec` startup path (Phase 3-owned) using `seeding.backend`/`seeding.model`
     - provide quality-grade evidence, repo intelligence profile summary (`codex_readiness` dimensions + `user_validated.additional_context` + `primary_gap`), conventions, architecture summaries, and codex-agent principles
     - seeding agent must treat `primary_gap` dimension as highest-leverage area; generated tasks must address root Codex readiness gaps, not just low-grade domains in isolation
     - require high-level, right-sized tasks with rationale and expected validation signal
     - persist input context (including profile snapshot) + output tasks for audit and reproducibility analysis.
     - parse output via shared `output_envelope` parser introduced in Phase 1.
     - enforce strict seeding response schema (`tasks[]`) and min/max task count contract.
     - mark this path as `legacy_seed_runner_v1`; Phase 6 must replace it with shared adapter trait and remove legacy path.
- Implement event-driven startup dispatch gate with precedence:
  - resumable assigned tasks
  - ready backlog tasks
  - pending external signals (PR/update deltas)
  - idle watch loop with escalation threshold.
- Add startup reconciliation observer that can close/recover stranded task state independently of worker runtime.
- Implement startup reconciliation and PR-upsert exactly per shared-foundation normative rules (worktree/task mismatch handling, PR keyed upsert, self-heal vs halt conditions).
- Produce startup health summary event:
  - quality-grades status
  - stale worktrees found/fixed
  - PR collisions found/fixed
  - backlog counts by priority.

### Success Criteria
- Startup loads repo intelligence profile before quality grade generation; missing profile always triggers hard stop with actionable error message — no fallback, no headless bypass.
- Profile `user_validated.validation_command` overrides startup config when no explicit config value is set.
- Quality document is readiness-first: headline score + grade, `## Triage Baseline` with profile path/`readiness_score`/`readiness_grade`/`primary_gap`, `## Agent Readiness` table from profile `[codex_readiness]`, `## Coverage Detail` with per-domain grades.
- `codex_readiness.has_coverage_gates = false` in profile causes coverage component to start at 0 for all domains (not inferred from absent data).
- Seeding prompt includes `codex_readiness` dimension summary and `primary_gap`; seeding agent output is tested against fixture that confirms `primary_gap` dimension addressed.
- Startup can recover from stale/hanging worktrees without deadlock.
- Missing quality grades path is handled deterministically.
- Stale-threshold detection is deterministic and config-driven (age + head-sha policy).
- `## Coverage Detail` section covers all discovered domains with deterministic grading output.
- Domain discovery and artifact mapping are explicit, drift-detected, and repository-agnostic.
- Scoped working-directory mode limits discovery/scoring/seeding to the configured subtree.
- Quality score mapping from evidence to grade is deterministic and reproducible across runs.
- Empty backlog seeding is agent-driven, auditable, and produces right-sized high-level tasks.
- Phase 3 seeding works before Phase 6 by using the direct startup runner path.
- Startup validation gate (when enabled) uses configured command resolution, not a hardcoded repo script.
- Startup health summary is emitted and persisted to JSONL.
- Startup dispatch precedence is deterministic and test-proven.
- Startup reconciliation can repair stranded task state before worker launch.

### Phase Validation Gate (Mandatory)
- Run: `cargo test -p gardener --all-targets`
- Run: `cargo llvm-cov -p gardener --all-targets --summary-only` (must report 100.00% lines for current `tools/gardener/src/**` code at this phase).
- Run E2E binary smoke: `scripts/brad-gardener --quality-grades-only --config tools/gardener/tests/fixtures/configs/phase03-startup-seeding.toml` and `scripts/brad-gardener --backlog-only --config tools/gardener/tests/fixtures/configs/phase03-startup-seeding.toml`.
- Run E2E binary smoke: `scripts/brad-gardener --working-dir tools/gardener/tests/fixtures/repos/scoped-app/packages/functions/src --quality-grades-only --config tools/gardener/tests/fixtures/configs/phase03-startup-seeding.toml` and `scripts/brad-gardener --working-dir tools/gardener/tests/fixtures/repos/scoped-app/packages/functions/src --backlog-only --config tools/gardener/tests/fixtures/configs/phase03-startup-seeding.toml`.

### Autonomous Completion Rule
- Continue directly to the next phase only after all success criteria and this phase validation gate pass.
- Do not wait for manual approval checkpoints.
