---
date: 2026-03-02
researcher: claude-opus
git_commit: a16d2ab
branch: main
topic: Friction analysis silent failure — null payload from agent adapter
tags: [friction-analysis, payload, parsing, agent-adapter, codex]
status: complete
---

# Research Question

Why does friction analysis silently fail with `parse_failed: "invalid type: null, expected struct FrictionAnalysisResponse"`, returning 0 findings on every run?

# Summary

Friction analysis constructs its `AdapterContext` with `output_schema: None` and `output_file: None`. Without an `--output-schema` flag, the Codex adapter's `turn.completed` event has no structured `"result"` field — the adapter falls back to `Value::Null` via `unwrap_or(Value::Null)`. Deserializing `Value::Null` as `FrictionAnalysisResponse` always fails. The error is caught and silently replaced with an empty default (`findings: [], smooth_run: false`), so 0 findings are registered every time.

Every other phase that successfully parses structured output from agents (seed runner, agent_turn doing phase) sets `output_schema` and/or `output_file`. Friction analysis sets neither.

# Detailed Findings

## 1. The null payload originates in the adapter

**Codex adapter** (`agent/codex.rs:272`):
```rust
let payload = done.get("result").cloned().unwrap_or(Value::Null);
```

**Claude adapter** (`agent/claude.rs:206-209`):
```rust
let payload = terminal_result
    .get("result")
    .cloned()
    .unwrap_or(Value::Null);
```

Without `--output-schema`, the Codex `turn.completed` event does not populate a `"result"` field with parsed JSON. The `unwrap_or(Value::Null)` produces a null `StepResult.payload`.

The Claude adapter has the same pattern, but the Claude `result` event includes the assistant's text in the `"result"` field (as `Value::String`). This would produce a different error ("invalid type: string") rather than "null". The observed "null" error confirms the **Codex backend** is being used (which matches `cfg.seeding.backend` defaulting to `AgentKind::Codex`).

## 2. Friction analysis does not set output_schema or output_file

`friction_analysis.rs:391-392`:
```rust
output_schema: None,
output_file: None,
```

Compare with:

**Seed runner** (`seed_runner.rs:104-105`):
```rust
output_schema: Some(output_schema),  // JSON Schema file for structured output
output_file: Some(output_file.clone()),  // Fallback file to read from
```

**Agent turn (doing phase)** (`agent_turn.rs:114-143`):
```rust
output_schema,  // Set for Doing state when adapter supports it
output_file: Some(output_file),
```

The seed runner also has a two-pass fallback: try event payload first, then read from the output file (`parse_seed_payload_from_file`). Friction analysis has no fallback.

## 3. The silent failure path

`friction_analysis.rs:409-426`:
```rust
let response: FrictionAnalysisResponse = match serde_json::from_value(result.payload.clone()) {
    Ok(r) => r,
    Err(e) => {
        append_run_log("warn", "friction_analysis.parse_failed", json!({...}));
        FrictionAnalysisResponse { findings: vec![], smooth_run: false }
    }
};
```

The error is logged as a warning but the function returns `Completed { findings: vec![] }`. In `worker.rs:1383-1384`, the empty findings vector hits `!findings.is_empty()` → false, so backlog task creation is skipped entirely.

## 4. Comparison: how other phases succeed

| Consumer | output_schema | output_file | Fallback strategy |
|---|---|---|---|
| Seed runner | Yes (JSON Schema) | Yes (.cache file) | Event payload → file read |
| Agent turn (Doing) | Yes (if adapter supports) | Yes | Direct parse, logs warning on failure |
| Agent turn (Understand) | No | Yes | Strict parse → keyword classifier fallback |
| Agent turn (Review) | No | Yes | Manual field extraction → default approve |
| **Friction analysis** | **No** | **No** | **None — returns empty default** |

Key insight: even phases without `output_schema` still set `output_file`, giving the adapter an `-o` flag that writes the last message to disk. The Codex adapter uses this file path regardless of whether `output_schema` is set (`codex.rs:94-97` — defaults to `.cache/gardener/codex-last-message.json`). However, friction analysis doesn't use the file at all in its parsing path.

# Root Cause

Missing `output_schema` configuration in friction analysis's `AdapterContext`. Without it, the Codex CLI doesn't produce structured JSON in the `turn.completed` event's `"result"` field, so the payload is null.

# Fix Options

## Option A: Add output_schema + output_file (Recommended)

Follow the seed runner pattern:
1. Define a JSON Schema for `FrictionAnalysisResponse` (inline string, like `seed_output_schema()`)
2. Write it to `.cache/gardener/schemas/friction_analysis_schema.json`
3. Set `output_schema: Some(schema_path)` and `output_file: Some(output_file)`
4. Add a file-read fallback like `parse_seed_payload_from_file`

**Pros**: Matches existing patterns, Codex enforces schema compliance, most robust.
**Cons**: Only works with Codex backend (Claude adapter ignores `output_schema`). But since `cfg.seeding.backend` defaults to Codex, this is fine.

## Option B: Parse from output_file only

1. Set `output_file: Some(path)` in the context
2. After execution, read the file and parse the JSON from it
3. Skip `output_schema` — the agent already has a clear prompt for the response format

**Pros**: Simpler, works with both backends.
**Cons**: No schema enforcement; agent might produce malformed JSON.

## Option C: Extract text from agent events

Scan `result.events` for assistant text content, extract the JSON from it.

**Pros**: No schema file needed.
**Cons**: Fragile — depends on event structure internals; different between Claude/Codex.
