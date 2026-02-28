---
name: backlog-db
description: Interact with Gardener backlog SQLite storage and create new backlog tasks.
---

# Backlog Database Skill

Use this skill when you need to inspect Gardener backlog rows or create new backlog tasks quickly.

## Script

- `scripts/backlog-db.sh`

## Common operations

- List the latest backlog rows:
  - `./scripts/backlog-db.sh list`
  - `GARDENER_DB_PATH=PATH ./scripts/backlog-db.sh list`

- Create a feature task:
  - `./scripts/backlog-db.sh add --title "GARD-xx: Your ticket" --details "What to fix" --priority P1 --scope runtime`

- Provide overrides:
  - `--kind`, `--status`, `--source`, `--id`, `--db`

## Required fields

- `--title` and `--details` are required for `add`.
- `priority` should be one of `P0`, `P1`, `P2`.
- If `--id` is omitted, generated task id is `manual:<scope>:auto-<unix_ms>`.

## Notes

- The script writes directly into `backlog_tasks` and expects an existing SQLite file.
- For large/interactive workflows, use the `run` command and `--report` options documented in `README.md`.
