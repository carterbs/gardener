# Backlog DB Rust CLI Plan

## Goal

Replace `scripts/backlog-db.sh` with a Rust CLI that behaves like `otel-logs`: first preserve current functionality, then add safe backlog inspection and edit operations.

## Current State

- `scripts/backlog-db.sh` currently supports `list`, `add`, `runbook`, and `help`.
- The helper defaults to `~/.gardener/backlog.sqlite`, with `--db` and `GARDENER_DB_PATH` overrides.
- `tools/gardener/src/bin/otel_logs.rs` already provides the preferred Rust CLI pattern for Gardener tools.
- `tools/gardener/src/backlog_store.rs` already owns the backlog schema and most state transitions, so the new CLI should route writes through store APIs instead of embedding raw SQL.

## Phase 1: Move Existing Functionality

### 1. Add a new Rust binary

- Add `[[bin]] name = "backlog-db"` to `tools/gardener/Cargo.toml`.
- Add `tools/gardener/src/bin/backlog_db.rs`.
- Mirror the `otel-logs` structure:
  - `Cli`
  - `Commands`
  - `run() -> Result<i32, GardenerError>`
  - `main()` that prints errors and exits non-zero

### 2. Preserve current path resolution

- Keep the current shell semantics for manual use:
  - `--db PATH` wins
  - else `GARDENER_DB_PATH`
  - else `~/.gardener/backlog.sqlite`
- Do not switch the manual CLI to runtime startup path resolution from `startup::backlog_db_path`.

### 3. Add a small support module

- Add `tools/gardener/src/backlog_cli.rs` for:
  - DB path resolution
  - input validation
  - row formatting
  - JSON output structs
  - shared error helpers

This keeps the binary thin and testable.

### 4. Migrate `list`

- Preserve current behavior:
  - show the latest 50 rows
  - fields: `task_id`, `title`, `priority`, `status`, `source`, `scope_key`
  - order by `created_at DESC`
- Do not reuse `BacklogStore::list_backlog_tasks()` for parity because it hides `merge_pending` and uses a different sort order.
- Add a dedicated read method to `BacklogStore` or a dedicated read-only query helper for CLI-compatible listing.

### 5. Migrate `add`

- Preserve current behavior:
  - required `--title` and `--details`
  - optional `--priority`, `--scope`, `--status`, `--kind`, `--source`, `--id`, `--db`
  - default task id format: `manual:<scope>:auto-<unix_ms>`
- Add a dedicated store API for manual task creation instead of copying shell SQL into the binary.
- Keep the same validation rules for `priority`, `status`, and `kind`.

### 6. Migrate `runbook` and `help`

- `runbook` should continue to print `docs/runbooks/backlog-operations.md`.
- `help` should be standard clap help with examples and env var notes.

### 7. Add parity tests before any new behavior

- Add `assert_cmd` tests for:
  - `backlog-db list`
  - `backlog-db add`
  - custom `--id`
  - invalid `--priority`
  - invalid `--status`
  - invalid `--kind`
  - `runbook`
- Use temp SQLite fixtures created through `BacklogStore::open()`.

### 8. Keep the shell entrypoint as a compatibility shim

- Reduce `scripts/backlog-db.sh` to a wrapper around:
  - `cargo run -q -p gardener --bin backlog-db -- "$@"`
- Keep this shim until docs and skills are fully migrated.

## Phase 2: Add New Functionality

### 9. Add `show`

- Purpose: inspect one exact task before editing it.
- Flags:
  - `--id` required
  - `--db`
  - `--json`
- Backing API:
  - `BacklogStore::get_task()`

### 10. Add `update`

- Purpose: support safe manual recovery without raw SQL.
- Initial writable fields:
  - `--status`
  - `--rationale`
  - `--related-pr`
  - `--related-branch`
  - `--clear-lease`
- Flags:
  - `--id` required
  - `--db`
  - `--dry-run`
  - `--json`
- Behavior:
  - refuse empty updates
  - print before/after state
  - validate status transitions at the CLI boundary

### 11. Extend `BacklogStore` for safe edits

- Add explicit write APIs rather than CLI-local SQL:
  - `insert_manual_task(...)`
  - `update_task_metadata(...)` or a small set of narrower update methods
  - possibly `retire_task(...)` for the stale-task case

This keeps schema ownership in one Rust module.

### 12. Add `retire`

- Purpose: provide a clear operator command for stale or already-complete manual tasks.
- Expected shape:
  - `backlog-db retire --id ... --status complete --rationale ... --related-pr 141 --related-branch ... --clear-lease`
- Behavior:
  - verify the task exists
  - clear lease state
  - update related PR metadata
  - set final status
  - print the final row

### 13. Add machine-readable output

- Mirror `otel-logs`:
  - `--json` on read commands
  - write commands emit a JSON object with `before`, `after`, and `changed`

This keeps the CLI usable from agents and scripts.

## Documentation and Adoption

### 14. Update docs and skill references

- Update:
  - `.codex/skills/backlog-db/SKILL.md`
  - `docs/runbooks/backlog-operations.md`
  - any README references
- Document the migration path:
  - old shell command still works
  - canonical interface becomes `cargo run -p gardener --bin backlog-db -- ...`

### 15. Decide whether to keep the shell shim

- After the Rust CLI is stable and adopted:
  - either keep the shell shim permanently for convenience
  - or delete it and point all instructions directly at the Rust binary

## Recommended Delivery Order

1. Add `backlog-db` binary skeleton and DB path resolver.
2. Implement `list`, `add`, `runbook`, and `help`.
3. Add parity tests.
4. Replace the shell script with a compatibility wrapper.
5. Implement `show`.
6. Implement `update`.
7. Implement `retire`.
8. Add JSON output polish.
9. Update docs and skill references.

## Key Constraint

Do not port the shell helper by copying raw SQL into the Rust CLI. Put the write semantics into `backlog_store.rs` first, then make the CLI a thin interface over those APIs. That avoids duplicating schema knowledge and keeps future backlog edits consistent with the rest of the runtime.
