---
date: 2026-03-01
researcher: codex
git_commit: 285a462
branch: main
topic: Claude backend parsing audit (claude.rs + protocol.rs)
tags: [claude, parsing, protocol, audit]
status: complete
---

# Research Question

Audit Claude backend parsing path across `tools/gardener/src/agent/claude.rs` and `tools/gardener/src/protocol.rs`, listing unstructured parsing assumptions and terminal status/result derivation.

# Summary

Claude runtime parsing is schema-light and `serde_json::Value`-based at all key decision points. The adapter parses stdout line-by-line as standalone JSON values and ignores non-JSON lines as diagnostics, then determines terminal outcome by scanning for the last event with top-level `type == "result"`.

Event classification for Claude is centralized in `map_claude_event`, where the only terminal discriminator is `subtype == "success"`; all other/missing subtypes are treated as failure. The adapter mirrors this logic independently when constructing `StepResult.terminal` and extracting `StepResult.payload` from `result`.

# Detailed Findings

## Ingestion and unstructured assumptions

- `claude.rs` parses each non-empty stdout line with `serde_json::from_str::<Value>`, assuming per-line JSON framing.
- Non-JSON stdout lines are explicitly ignored (with diagnostics), meaning unstructured text does not fail the run if a terminal result event exists.
- `protocol.rs` utilities (`parse_jsonl`, `parse_json_records`) are stream-oriented but line-split first in `parse_jsonl`; they do not support pretty-printed JSON objects spanning lines.

## Terminal derivation

- `claude.rs` finds terminal from the last raw event whose top-level `type` equals `"result"`.
- Terminal state is `Success` only when `subtype == "success"`; otherwise `Failure`.
- Step payload is copied from top-level `result` field of that terminal event, defaulting to JSON null if absent.
- If terminal result exists, the adapter returns `StepResult` without checking process exit code.

# Code References

| File | Lines | Description |
|------|-------|-------------|
| `tools/gardener/src/agent/claude.rs` | 128-173 | Stdout NDJSON-by-line parse, non-JSON ignore path |
| `tools/gardener/src/agent/claude.rs` | 201-243 | Terminal event lookup, subtype->terminal mapping, payload extraction |
| `tools/gardener/src/agent/claude.rs` | 245-280 | Fallback behavior when no terminal result event |
| `tools/gardener/src/protocol.rs` | 64-90 | Claude event type mapping and subtype-based completion/failure mapping |
| `tools/gardener/src/protocol.rs` | 92-111 | Generic JSONL/JSON stream parsing helpers |

# Architecture Insights

Claude parsing is intentionally tolerant of mixed stdout content and unknown event variants, favoring forward compatibility and runtime resilience over strict schema enforcement. Terminal correctness depends on presence and shape of a final `result` event.

# Historical Context

Not investigated in this audit.

# Open Questions

- Should a non-zero exit code override a terminal `result` event to prevent false-success when process exits abnormally?
- Should missing/non-object `result` payloads be treated as protocol errors instead of `Value::Null`?
