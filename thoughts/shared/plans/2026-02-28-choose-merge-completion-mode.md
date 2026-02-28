# Allow users to choose local merge vs PR-per-task completion

**Date:** 2026-02-28  
**Scope:** `tools/gardener/src/` and runtime docs/config templates  
**Input:** user request to choose between local merging and one PR per task completion

---

## Overview

Users need a first-class option to pick how a completed task is delivered:
- local merge path (single local branch merge per task), or
- PR-per-task path (open and merge a PR per task completion).

Current behavior ties merge strategy to `execution.git_output_mode` (`PullRequest` vs `CommitOnly|Push`), which makes merge strategy implicit and under-documented.

## Current State Analysis

- `execution.git_output_mode` exists and defaults to `PullRequest` in defaults.
  - `tools/gardener/src/config.rs:114-136`, `tools/gardener/src/config.rs:196`
- `git_output_mode` is optional in partial config overrides and applied on load.
  - `tools/gardener/src/config.rs:288-289`, `tools/gardener/src/config.rs:513-514`
- Worker construction always feeds the same field into both gitting and merging prompt selection.
  - `tools/gardener/src/worker.rs:122-123`
- Merging prompt selection is currently coupled in prompt registry:
  - `tools/gardener/src/prompt_registry.rs:41-44` (`PullRequest` -> PR merge template; `CommitOnly|Push` -> local merge template)
- CLI has no merge-style argument today.
  - `tools/gardener/src/lib.rs` defines runtime flags and maps into `CliOverrides` without merge-style.
- Config/docs have no documented `git_output_mode` usage, and fixture files do not set merge mode explicitly.
  - `tools/gardener/tests/fixtures/configs/phase10-full.toml:23-26` (contains only `permissions_mode`, `worker_mode`, `test_mode`)
  - `docs/guides/gardener-orchestrator.md` has no merge-delivery configuration section.

## Desired End State

- Add an explicit execution option (config + optional CLI override) that controls only completion delivery:
  - `local` (or equivalent) = local merge path
  - `pr_per_completion` (or equivalent) = one PR per completed task
- Keep existing `git_output_mode` semantics for gitting behavior.
- Preserve compatibility with existing `PullRequest` default behavior unless users opt in to local mode.
- Document the behavior and precedence clearly.

## What We're NOT Doing

- Changing `git_output_mode` values or names.
- Changing merge command flags (`--merge` vs `--squash`/`--rebase`) or branch naming strategy.
- Reworking PR conflict handling, validation strategy, or worker retry policy in this task.

## Phase 1: Add explicit merge completion mode to config + CLI

### Changes required

- Add a new config enum, e.g. `MergeCompletionMode` with serde values like `local` and `pr_per_completion` to `tools/gardener/src/config.rs`.
- Add `merge_mode: MergeCompletionMode` to `ExecutionConfig`, defaulting to `pr_per_completion` (or keep equivalent of current PR behavior).
- Extend partial config handling and config load merge logic to accept `[execution].merge_mode`.
- Extend CLI with `--merge-mode` (optional) and include in `CliOverrides`.
- Apply CLI override in `run()`/`load_config()` flow.

### Success criteria

- Config file and CLI can both specify merge completion mode.
- Existing configs without `merge_mode` retain current behavior.

### Confirmation gate

- Config round-trip/load test with and without the new key passes.

## Phase 2: Wire merge strategy independently of gitting mode

### Changes required

- Update prompt registry API so merging style is driven by `MergeCompletionMode` (not `GitOutputMode`):
  - Add/rename method like `with_merge_completion_mode(...)` or update existing `with_merging_mode(...)`.
  - Preserve current gitting behavior (still from `git_output_mode`) at `worker.rs` call site.
- In `tools/gardener/src/worker.rs`, pass merge mode separately from gitting mode:
  - `with_gitting_mode(&cfg.execution.git_output_mode)` remains unchanged.
  - merging prompt selection uses `cfg.execution.merge_mode`.

### Success criteria

- `PullRequest`-style PR completion and `local` completion are deterministic and independent from `CommitOnly` vs `Push`.

### Confirmation gate

- Manual run with equivalent gitting settings and different completion modes produces expected prompt templates.

## Phase 3: Validate mode behavior and document

### Changes required

- Add tests around config + prompt registry mode routing:
  - registry emits PR merge template when merge mode is `pr_per_completion`.
  - registry emits local merge template when merge mode is `local`.
- Add/extend integration tests to verify merge completion mode plumbing from config/CLI to worker run config selection.
- Add docs/config sample snippet for:
  - `execution.merge_mode = "local"` and
  - `execution.merge_mode = "pr_per_completion"`
- Add note in runtime docs that this is the user choice point for completion delivery.

### Success criteria

- Coverage exists for both config and CLI overrides of merge mode.
- Docs show an explicit example and default behavior.

### Confirmation gate

- Test matrix includes at least:
  - default config (no merge_mode) → PR-per-completion
  - local mode via config
  - local mode via CLI

## Testing Strategy

- Unit tests: config parsing and prompt template selection by mode.
- Integration tests: worker path selection for each merge mode.
- Manual smoke:
  - run with `--merge-mode local` and confirm local merge path is selected,
  - run with `--merge-mode pr_per_completion` and confirm PR merge path is selected.

## References

- `tools/gardener/src/config.rs`
- `tools/gardener/src/worker.rs`
- `tools/gardener/src/prompt_registry.rs`
- `tools/gardener/src/lib.rs`
- `tools/gardener/tests/fixtures/configs/phase10-full.toml`
- `docs/guides/gardener-orchestrator.md`
