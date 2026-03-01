---
date: 2026-03-01
researcher: codex
git_commit: 285a462
branch: main
topic: Worker FSM Claude-path output parsing audit
tags: [worker, fsm, claude, parsing]
status: complete
---

# Research Question

Where in the worker finite state machine and Claude backend path are we still parsing agent output, and which areas are brittle given Claude does not return guaranteed structured outputs?

# Summary

The live worker FSM is orchestrated in `tools/gardener/src/worker.rs` and delegates each state turn (`understand`, `planning`, `doing`, `gitting` remediation, `reviewing`, `merging` remediation) through `run_agent_turn`. For Claude-backed states, payloads come from `ClaudeAdapter::execute`, which parses stream-json lines and returns the `result` object from the last terminal Claude `type:"result"` event.

There is no strict schema enforcement for live Claude payloads in the worker FSM. Structured envelope parsing (`parse_typed_payload`) is present, but only used in simulated/test validation paths, not on live Claude turn outputs. Live state progression still depends on permissive field extraction / fallback logic (`parse_understand_output`, `parse_reviewing_output`, and `extract_failure_reason`) that can silently coerce malformed or partial payloads.

# Detailed Findings

## Worker FSM state parsing points

`execute_task_live` consumes `TurnResult.payload` and applies state-specific parsing at three key points:

- UNDERSTAND: `parse_understand_output` tries `serde_json::from_value::<UnderstandOutput>` and falls back to keyword classification if payload shape is wrong.
- REVIEWING: `parse_reviewing_output` reads optional `verdict` and `suggestions` fields with permissive defaults (`approve` + empty list).
- Terminal failure handling in multiple states: `extract_failure_reason` extracts `reason`/`message`, and if stringified JSON is detected it tries to read nested `detail`.

These parsers directly influence FSM transitions (`apply_understand`, review loop branching, failed terminal result paths).

## Claude adapter parsing contract

`ClaudeAdapter::execute` parses stdout line-by-line as JSON, maps events, then identifies terminal state by scanning for the last event with `type == "result"`.

- Terminal outcome is inferred from `subtype == "success"`; anything else is failure.
- Payload returned to worker is the raw `result` field from that event (or `Null` if missing).
- Non-JSON stdout lines are ignored with diagnostics, not fatal if a terminal result exists.

This means the worker receives weakly validated `serde_json::Value` payloads and performs downstream, state-specific parsing/coercion.

## Structured envelope parser is not used for live worker turns

`output_envelope::parse_typed_payload` and marker constants are imported in `worker.rs`, but the only invocations in that file are:

- A prompt-validation sanity check in `prepare_prompt` using synthetic envelope text.
- Simulated execution path (`execute_task_simulated`) with hardcoded payload.

No live Claude FSM turn currently enforces envelope markers or typed payload schema before transition logic.

# Code References

| File | Lines | Description |
|------|-------|-------------|
| `tools/gardener/src/worker.rs` | 169-215 | UNDERSTAND turn result consumed and parsed into `UnderstandOutput` |
| `tools/gardener/src/worker.rs` | 430-516 | REVIEWING turn result parsed and used for needs-changes vs merge transition |
| `tools/gardener/src/worker.rs` | 78-91 | Failure reason extraction logic (`reason`/`message` and JSON-in-string parse) |
| `tools/gardener/src/worker.rs` | 1107-1156 | `parse_understand_output` and `parse_reviewing_output` implementations |
| `tools/gardener/src/worker.rs` | 1078-1086 | `parse_typed_payload` used only as synthetic prompt parser check |
| `tools/gardener/src/worker.rs` | 749-754 | `parse_typed_payload` usage in simulated path only |
| `tools/gardener/src/agent/claude.rs` | 128-173 | Claude stdout JSON line parsing with non-JSON ignore behavior |
| `tools/gardener/src/agent/claude.rs` | 201-243 | Terminal event selection and payload extraction from `result` event |
| `tools/gardener/src/protocol.rs` | 64-90 | Claude event type mapping (`result` subtype drives completed/failed classification) |
| `tools/gardener/src/output_envelope.rs` | 15-60 | Strict envelope parser (available but not applied to live worker turns) |

# Architecture Insights

The runtime architecture separates adapter-level transport parsing (Claude stream-json to `StepResult`) from worker-level semantic parsing (state payload interpretation). This separation keeps adapter logic generic but currently leaves FSM correctness dependent on permissive JSON field extraction in worker state handlers.

The deterministic FSM transitions in `fsm.rs` are strict, but the data entering those transitions from live Claude turns is not strictly typed/validated.

# Historical Context

Current tests codify permissive behavior (e.g., default approve without verdict; invalid understand payload falls back to classifier; Claude result without result field can yield `Null`). This suggests the implementation intentionally prioritized resilience to noisy output over strict contract enforcement.

# Open Questions

- Should live worker turns require envelope markers + typed schema per state before transition decisions?
- Should malformed REVIEWING payloads fail closed instead of defaulting to approve?
- Should Claude terminal success with missing/invalid `result` be treated as failure in adapter layer?
