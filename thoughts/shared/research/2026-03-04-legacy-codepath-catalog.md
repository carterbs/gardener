---
date: 2026-03-04
researcher: codex
git_commit: 9429e170b6fe435f016d0527fe309b3c999d61cc
branch: main
topic: Legacy/backward-compatibility codepath catalog in runtime Rust code
tags: [legacy, compatibility, runtime, catalog]
status: complete
---

# Research Question

Catalog places in the active Rust runtime that preserve legacy/backward-compatibility behavior, without making code changes.

# Summary

The runtime contains several explicit legacy/back-compat codepaths in active execution, mostly in three clusters: CLI/config migration shims, seeding v1/v2 transitional paths, and event/log schema compatibility handling.

The highest-signal legacy paths are: deprecated CLI alias support (`--worker-count`), startup env/config fallbacks that read old keys/env vars, explicit fallback to the old quality renderer, and active use of a `run_legacy_seed_runner_v1...` path for interactive review/dry-run seeding.

There are also compatibility normalizers that may be intentional for robustness (state alias normalization and multi-shape event field extraction). These still codify old shapes/labels and are worth evaluating against your “no backwards-compat” stance.

# Detailed Findings

## 1. CLI + config compatibility shims

- Deprecated CLI alias still supported and warned:
  - `--worker-count` remains a valid arg, mapped into `num_workers`.
  - Warning indicates planned removal.
- Validation command resolution preserves old config location:
  - Falls back from `validation.command` to legacy `startup.validation_command`.
- Startup can inherit validation command from profile (`profile.user_validated.validation_command`) when startup config is missing.

## 2. Legacy quality rendering fallback

- Startup quality refresh tries new pipeline first, then explicitly falls back to a legacy profile-based renderer with a `pipeline_fallback` warning reason containing “legacy profile-based renderer”.

## 3. Seeding v1/v2 transitional compatibility

- Seeding includes explicit legacy runner APIs (`run_legacy_seed_runner_v1*`).
- Interactive seeding branches into:
  - review mode: v1 dry-run + review wizard
  - auto mode: v2 direct-write path
- Seed parsing supports both payload formats:
  - direct `{"tasks":[...]}`
  - envelope `{"schema_version":...,"state":...,"payload":{"tasks":[...]}}`
- If event payload parse fails, code falls back to parsing an output file.
- Prompt registry still carries legacy/direct aliases (`SEEDING_PROMPT_VERSION_LEGACY`, `SEEDING_PROMPT_VERSION_DIRECT`) plus old wrapper funcs (`seeding_v2_prompt_template`, `seeding_v3_direct_prompt_template`) that now both return the same template.

## 4. Event/log compatibility handling

- Protocol mapping collapses old/new event names into unified internal kinds (e.g. `item.started` + `item.updated` both map to `ToolCall`; `turn.failed` + `error` map to `TurnFailed`; multiple Claude event variants normalized).
- Friction analysis supports both current nested OTEL payload (`gardener.payload`) and legacy flat attributes (`payload.*`, `payload.worker_id`).
- Seed UI event summarization extracts labels/commands from multiple field variants (`/item/...`, `/tool_name`, `/command_line`, etc.).

## 5. Transitional worker-state normalization

- Worker-state transition checks normalize many alias labels (`init`, `boot`, `working`, `commit`, `pr_creating`, etc.) into current FSM buckets before ranking transitions.

# Code References

| File | Lines | Description |
|------|-------|-------------|
| `tools/gardener/src/lib.rs` | 114, 233, 249-251 | Deprecated `--worker-count` alias still parsed and warned |
| `tools/gardener/src/config.rs` | 692-699 | Validation command fallback to legacy `config.startup` key |
| `tools/gardener/src/startup.rs` | 31, 52-57 | Legacy backlog DB env var (`GARDENER_DB_PATH`) fallback |
| `tools/gardener/src/startup.rs` | 87-107 | New quality pipeline fallback to legacy renderer |
| `tools/gardener/src/startup.rs` | 334-347 | Startup config inherits validation command from profile |
| `tools/gardener/src/startup.rs` | 1056-1058, 1096-1100, 1202-1204 | Interactive seeding routes between v1 review path and v2 direct path |
| `tools/gardener/src/startup.rs` | 1329-1340, 1381-1404 | Multi-shape event field extraction fallback |
| `tools/gardener/src/seeding.rs` | 10-12, 217-227, 283-293, 352 | Active mixed v1 legacy runner + v2 direct runner paths |
| `tools/gardener/src/seed_runner.rs` | 49-67, 86-106 | Explicit legacy seed runner API surface |
| `tools/gardener/src/seed_runner.rs` | 214-237, 364-370 | Parse fallback: event payload -> file payload; direct+envelope payload compatibility |
| `tools/gardener/src/prompt_registry.rs` | 16-18, 115-121 | Legacy/direct prompt-version aliases and old wrapper functions |
| `tools/gardener/src/protocol.rs` | 46-53, 67-83 | Old/new event-type normalization for Codex/Claude events |
| `tools/gardener/src/friction_analysis.rs` | 203-206, 247-274 | Legacy flat OTEL attribute compatibility with current nested format |
| `tools/gardener/src/worker_pool.rs` | 1539-1557 | Normalization of older/transitional worker-state labels |

# Architecture Insights

- Compatibility is concentrated at system boundaries: CLI parsing, config resolution, agent protocol parsing, and telemetry parsing.
- Seeding is currently the strongest explicit migration zone, with both legacy (JSON-returning dry-run) and newer direct-write flows coexisting.
- Quality-report generation still keeps a hard fallback to old renderer logic, indicating an incomplete cutover.

# Historical Context

- The naming/comments indicate active migration from earlier paths (legacy profile-based quality rendering, seeding v1 to direct-v2, old flag and env var names).
- Several compatibility branches include explicit log messages/comments referencing deprecation or legacy behavior rather than implicit generic fallback.

# Open Questions

- Should all legacy compatibility be removed immediately, or is there a narrow runtime boundary you still want to keep tolerant (e.g., agent event schema drift)?
- Is profile inheritance for `validation_command` considered legacy compatibility, or still part of the intended product contract?
