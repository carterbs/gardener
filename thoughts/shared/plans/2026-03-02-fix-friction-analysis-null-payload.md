# Fix friction analysis null payload — add output_schema + file fallback

## Context

Friction analysis silently fails on every run. The agent produces a valid text response, but `run_friction_analysis` sets `output_schema: None` and `output_file: None` in its `AdapterContext`. Without `--output-schema`, the Codex CLI's `turn.completed` event omits the `"result"` field, so the adapter returns `Value::Null`. Deserializing null as `FrictionAnalysisResponse` fails, the error is caught, and an empty default is returned — 0 findings every time.

The fix follows the established seed runner pattern: write a JSON Schema file, pass it as `output_schema`, set an `output_file`, and add a file-read fallback for robustness.

## Files to modify

1. **`tools/gardener/src/friction_analysis.rs`** — main changes
2. **`tools/gardener/src/bin/friction_analysis.rs`** — CLI binary (if it needs schema/file alignment)

## Implementation

### Step 1: Add JSON Schema function

Add `friction_output_schema() -> String` to `friction_analysis.rs`, returning a JSON Schema for `FrictionAnalysisResponse`:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["findings", "smooth_run"],
  "properties": {
    "findings": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["category", "title", "description", "severity", "evidence_events"],
        "properties": {
          "category": { "type": "string" },
          "title": { "type": "string", "minLength": 5 },
          "description": { "type": "string", "minLength": 10 },
          "severity": { "type": "string", "enum": ["high", "medium", "low"] },
          "evidence_events": { "type": "array", "items": { "type": "string" } }
        }
      }
    },
    "smooth_run": { "type": "boolean" }
  }
}
```

### Step 2: Add schema path helper

Add `friction_output_schema_path(scope: &RuntimeScope) -> Result<PathBuf, GardenerError>`:
- Path: `.cache/gardener/schemas/friction_analysis_schema.json`
- Pattern: check-before-write (match `seed_output_schema_path` at `seed_runner.rs:341-364`)

### Step 3: Add file-read fallback parser

Add `parse_friction_response_from_file(path: &Path) -> Result<FrictionAnalysisResponse, GardenerError>`:
- Read file, trim, parse JSON
- Match the `parse_seed_payload_from_file` pattern (`seed_runner.rs:301-339`)

### Step 4: Update `run_friction_analysis` AdapterContext

In `run_friction_analysis` (line ~383), change:
```rust
// Before
output_schema: None,
output_file: None,

// After
output_schema: Some(friction_output_schema_path(scope)?),
output_file: Some(output_file.clone()),
```

Where `output_file` is `.cache/gardener/friction-analysis-{worker_id}.json` with `create_dir_all` for the parent.

### Step 5: Add two-pass parsing

Replace the single `serde_json::from_value` at line 409 with a two-pass strategy:

1. Try `serde_json::from_value::<FrictionAnalysisResponse>(result.payload.clone())`
2. On failure, try `parse_friction_response_from_file(&output_file)`
3. On both failures, log warning with both errors and return the existing empty default

### Step 6: Add tests

Add to the existing `#[cfg(test)] mod tests`:
- `friction_output_schema_is_valid_json` — parse the schema string as `serde_json::Value`
- `parse_friction_response_from_file_success` — write valid JSON to a temp file, parse it
- `parse_friction_response_from_file_missing` — confirm error on missing file
- `parse_friction_response_from_file_invalid_json` — confirm error on bad JSON

## Verification

```bash
# Unit tests
cargo test -p gardener friction_analysis

# Full test suite (ensure no regressions)
cargo test -p gardener

# Compile check
cargo check -p gardener
```
