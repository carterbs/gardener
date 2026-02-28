# Domain Quality Grades

Last updated: 2026-02-27

## Grading Methodology

Grades are based on four dimensions:

1. **Coverage Strength (Primary)** - Line coverage per domain. 95%+ is top tier, 90-94% strong, 80-89% solid, below 80% degrades quickly. Global 90% gate is enforced by CI via `cargo-llvm-cov`.
2. **Test Quality** - Integration test presence, assertion depth, and whether tests exercise real behavior vs trivial stubs.
3. **Structural Completeness** - Whether the domain has proper error handling, logging instrumentation, and config surface.
4. **Agent Legibility** - Whether the domain's interfaces, prompts, and docs are legible enough for agents to reason about and extend.

**Grade scale:**
- **A** - Coverage is excellent and quality/completeness checks are strong.
- **B** - Coverage is good but quality/completeness has a meaningful gap.
- **C** - Coverage or quality is weak, or multiple completeness gaps exist.
- **D** - Significant gaps across multiple dimensions. Feature works but is fragile.
- **F** - Broken or non-functional.

---

## Domain Grades

| Domain | Grade | Unit Tests | Integration Tests | Instrumentation | Agent Legible | Notes |
|--------|-------|------------|-------------------|-----------------|---------------|-------|
| Triage / Repo Intelligence | **A** | Yes (5 files) | `phase02b`, `phase08` | Yes | Yes | Full triage pipeline covered: detection, discovery, interview, profile persistence. Two dedicated integration test files. |
| Backlog Store | **A** | Yes (large suite) | Via `phase03`, `phase08` | Yes | Yes | SQLite queue with lease/claim semantics. Large inline test suite covers upsert, claim, lease recovery, snapshots. |
| Agent Adapters | **A** | Yes | `phase05` | Yes | Yes | Claude + Codex adapters with dedicated integration tests covering happy path, turn-failed, malformed events, discovery envelopes. |
| TUI / Hotkeys | **A** | Yes | `phase06`, `phase07`, `hotkey_lint`, `hotkey_pty_e2e` | N/A | N/A | 4 test files including a completeness linter and PTY end-to-end. Best-tested domain. |
| Worker Pool / Scheduler | **B** | Yes (both files) | Implicit via FSM | Yes | Partial | Hotkey behaviors and slot limiting tested inline. Missing dedicated integration tests for the full claim→execute→complete cycle. |
| Quality Grades | **B** | Yes (scoring, evidence) | Via `phase03` | Yes | Partial | Scoring and evidence collection tested. Domain catalog is hardcoded to return `"core"` — not a real domain discovery. |
| Startup / Reconciliation | **B** | Yes | `phase03` | Yes | Partial | Quality doc generation and missing-profile guard tested. Worktree/PR reconciliation lacks dedicated tests. |
| Logging / Infrastructure | **B** | Yes | `instrumentation_lint` | Self | Yes | Instrumentation linter enforces 90% coverage. Config loading, error types, log retention all have inline tests. |
| Git / GitHub Integration | **C** | Yes (inline) | None | Partial | No | Inline unit tests only. No integration tests. Real `git`/`gh` calls are hard to test but could use process runner fakes. |
| Prompts / Context / Knowledge | **C** | Yes (inline) | None | Partial | No | Prompt rendering, context items, and registry have inline tests. No tests for prompt quality or completeness. Agent legibility of prompts is poor. |
| Learning / Post-merge | **C** | Yes (inline) | None | Partial | No | Learning loop, post-merge analysis, and postmortem all have inline tests but no integration coverage. Interfaces are underspecified. |
| Seeding | **D** | Minimal | Via `phase03` (gate only) | Yes | No | Prompt is a single sentence with two numbers. No repo inspection, no article reference, no quality doc analysis. Fallback tasks are hardcoded templates. Agent cannot reason about what the repo actually needs. |

---

## Test File Inventory

### Integration Tests (`tools/gardener/tests/`)

| File | Coverage |
|------|----------|
| `phase02b_triage.rs` | Non-interactive detection, agent detection on fixtures, `run_triage`, `triage_needed` |
| `phase03_startup.rs` | Quality doc generation from profile, hard-stop when profile missing, seeding gate |
| `phase05_agent_adapters.rs` | Claude/Codex happy path, turn-failed, malformed events, discovery envelopes |
| `phase06_tui_rendering.rs` | Dashboard header, worker states, zombie indicator, backlog badges, all screens |
| `phase07_hotkeys.rs` | Standard bindings, operator gating, unknown keys, legend completeness |
| `phase08_triage_integration.rs` | Triage decision logic, envelope parsing, non-interactive guards |
| `hotkey_lint.rs` | Advertised-vs-behavior completeness linter |
| `hotkey_pty_e2e.rs` | PTY end-to-end hotkey tests |
| `instrumentation_lint.rs` | Per-file `append_run_log` instrumentation coverage >= 90% |
| `tui_harness_test.rs` | TUI harness integration |
| `worker_log_payload_linter.rs` | Worker log payload format enforcement |

### Unit Test Modules (inline `#[cfg(test)]`)

30 of 48 source files contain inline test modules. Notable:
- `backlog_store.rs` — largest inline suite (lease semantics, concurrent claims, recovery)
- `worker_pool.rs` — hotkey behaviors, slot limiting, FSM lifecycle
- `worker.rs` — FSM state transitions, output envelope parsing
- `config.rs` — config loading, model validation, defaults
- `repo_intelligence.rs` — profile serialization, readiness derivation

---

## Active Tech Debt

### Seeding System Overhaul
- [ ] **Seed prompt is non-functional** — Current prompt is `"Seed backlog tasks for primary_gap=X with readiness_score=Y.\nUse evidence:\n{quality_doc}"`. Agent has no context about what makes a good task, what the repo looks like, or what to optimize for.
- [ ] **No repo inspection in seeding** — Agent is not told to read the codebase. It seeds blindly from two numbers and a quality doc.
- [ ] **Fallback tasks are hardcoded** — Three cycling template strings ("Bootstrap backlog", "Stabilize validation loop", "Rank quality risks") with no domain-specific reasoning.
- [ ] **SeedTask.rationale is discarded** — The `rationale` field is parsed from agent output but never stored in the backlog.

### Quality Grades System
- [ ] **Single hardcoded domain** — `quality_domain_catalog.rs` returns `["core"]`. Should discover real domains (triage, backlog, worker, etc.).
- [ ] **Scoring is crude** — Base 80 + 5/tested_file - 2/untested_file. No assertion density, no integration test weighting, no domain-specific criteria.
- [ ] **No tech debt tracking** — Quality doc has no section for tracking active debt or recently completed improvements.

### Prompt System
- [ ] **No structured prompt for seeding** — Unlike worker FSM states which have `render_state_prompt`, seeding uses a raw `format!()` string.
- [ ] **Prompt knowledge not used in seeding** — `prompt_knowledge.rs` exists but seeding bypasses it entirely.

### Test Gaps
- [ ] **No integration tests for Git/GitHub** — `git.rs`, `gh.rs`, `worktree.rs` rely only on inline unit tests.
- [ ] **No integration tests for Learning Loop** — `learning_loop.rs`, `postmerge_analysis.rs`, `postmortem.rs` have no integration coverage.
- [ ] **Worker pool lacks end-to-end test** — No test exercises the full claim→dispatch→execute→complete cycle.
