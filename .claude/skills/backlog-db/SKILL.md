---
name: backlog-db
description: Interact with Gardener backlog SQLite storage and create new backlog tasks.
---

# Backlog Database Skill

Use this skill when you need to inspect Gardener backlog rows or create new backlog tasks quickly.

## CLI

- Canonical: `cargo run -q -p gardener --bin backlog-db --`
- Compatibility shim: `scripts/backlog-db.sh`

## Common operations

- List the latest backlog rows:
  - `cargo run -q -p gardener --bin backlog-db -- list`
  - `GARDENER_DB_PATH=PATH cargo run -q -p gardener --bin backlog-db -- list`

- Create a feature task:
  - `cargo run -q -p gardener --bin backlog-db -- add --title "GARD-xx: Your ticket" --details "What to fix" --priority P1 --scope runtime`

- Provide overrides:
  - `--kind`, `--status`, `--source`, `--id`, `--db`

- Inspect or safely edit an exact row:
  - `cargo run -q -p gardener --bin backlog-db -- show --id TASK_ID`
  - `cargo run -q -p gardener --bin backlog-db -- update --id TASK_ID --status complete --rationale "manual recovery" --clear-lease`
  - `cargo run -q -p gardener --bin backlog-db -- retire --id TASK_ID --status failed --rationale "duplicate"`

## Runbook

- `cargo run -q -p gardener --bin backlog-db -- runbook` prints `docs/runbooks/backlog-operations.md`.
- `./scripts/backlog-db.sh runbook` still works via the compatibility shim.
- Use this runbook for common manual operations, state glossary, and recovery guidance.

## Required fields

- `--title` and `--details` are required for `add`.
- `priority` should be one of `P0`, `P1`, `P2`.
- `--kind` must be one of: `feature`, `maintenance`, `quality_gap`, `bugfix`, `infra`, `merge_conflict`, `pr_collision`. **Always use snake_case** — the Rust parser rejects PascalCase (e.g. `QualityGap` will cause a database conversion error).
- `--status` should be one of: `ready`, `leased`, `in_progress`, `merge_pending`, `complete`, `failed`, `unresolved`.
- If `--id` is omitted, generated task id is `manual:<scope>:auto-<unix_ms>`.

## Notes

- The Rust CLI routes writes through `BacklogStore`; it does not write raw SQL from the CLI layer.
- Manual CLI operations still expect an existing SQLite file.
- For large/interactive workflows, use the `run` command and `--report` options documented in `README.md`.
