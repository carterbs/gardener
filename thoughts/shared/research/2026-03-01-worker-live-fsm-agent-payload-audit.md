---
date: 2026-03-01
researcher: codex
git_commit: be8f2c4
branch: main
topic: Live worker FSM agent payload parsing/coercion points in worker.rs
tags: [worker, fsm, payload, parsing]
status: complete
---

# Research Question

Where in `tools/gardener/src/worker.rs` does live-path agent output get parsed or coerced before finite-state transitions, and how brittle are those points?

# Summary

In live execution (`execute_task_live`), `TurnResult.payload` is carried as raw `serde_json::Value` and only parsed/coerced at three worker-level helpers: `parse_understand_output`, `parse_reviewing_output`, and `extract_failure_reason`. Of these, only understand/review parsing directly influence state transitions.

`parse_understand_output` is strict-first (`serde_json::from_value`) then falls back to deterministic keyword classification from task summary if payload shape is invalid. `parse_reviewing_output` is permissive and defaults to approval in the absence of a recognized verdict. Failure payloads are loosely interpreted for logging/reporting via `extract_failure_reason` and do not invoke FSM transition APIs; they short-circuit to `WorkerState::Failed` in returned summary.

# Detailed Findings

## Understand payload parse -> transition

- Callsite: `parse_understand_output(&understand_result.payload, ...)` then `fsm.apply_understand(&understand)`.
- Transition impact: `apply_understand` computes `Understand -> Planning|Doing` from `task_type`.
- Coercion behavior: invalid payload skips schema failure and falls back to `classify_task(task_summary)` keyword heuristics.
- Brittleness: malformed or partially wrong payload silently changes planning behavior based on task summary text, not agent intent.

## Reviewing payload parse -> transition

- Callsite: `parse_reviewing_output(&reviewing_result.payload)`.
- Transition impact: `NeedsChanges` loops to `Doing`; otherwise transitions to `Merging`.
- Coercion behavior:
  - `verdict`: case-folded string; only `needs_changes` maps to `NeedsChanges`; everything else becomes `Approve`.
  - `suggestions`: non-array or non-string entries are dropped.
- Brittleness: typoed/novel verdict values fail open to approval, potentially advancing to merge.

## Failure payload parse (terminal short-circuit)

- Callsites across understand/planning/doing/gitting/remediation/reviewing/merging remediation terminal failure branches.
- Coercion behavior:
  - reads `reason` or `message` string,
  - if string itself is JSON, attempts parse and extracts nested `detail`.
- Transition impact: none via FSM APIs; caller returns summary with `final_state: Failed`.
- Brittleness: non-string reason/message are ignored (`None`), and nested JSON parse is best-effort only.

# Code References

| File | Lines | Description |
|------|-------|-------------|
| `tools/gardener/src/worker.rs` | 202, 215 | Understand payload parsed then applied to FSM transition |
| `tools/gardener/src/worker.rs` | 1107-1131 | Understand parse/coercion + fallback classifier |
| `tools/gardener/src/worker.rs` | 466, 468-469, 502, 515 | Reviewing payload parsed and used for transition decision |
| `tools/gardener/src/worker.rs` | 1133-1156 | Reviewing parse/coercion defaults |
| `tools/gardener/src/worker.rs` | 78-91 | Failure payload reason extraction/coercion |
| `tools/gardener/src/worker.rs` | 184, 233, 269, 400, 447, 628 | Failure reason extraction callsites before failed return |
| `tools/gardener/src/worker.rs` | 861-864, 993-996 | Payload remains untyped JSON from adapter to worker FSM logic |

# Architecture Insights

Worker FSM decisions are primarily deterministic and only minimally data-driven from agent payloads in live path: understand category and review verdict. Everything else in live orchestration is deterministic git/GH orchestration with payload mostly for diagnostics/failure reporting.

# Historical Context

No commit-history deep dive performed for this audit; findings are from current `main` at `be8f2c4`.

# Open Questions

- Should review verdict parsing fail closed (invalid => `NeedsChanges`) instead of fail open to `Approve`?
- Should understand fallback classifier be gated behind explicit config or confidence checks to avoid silent route changes?
