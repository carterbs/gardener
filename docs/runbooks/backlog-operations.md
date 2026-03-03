# Backlog Operations Runbook for Agents

This runbook documents manual backlog actions for agents.

## Backlog path split

The backlog lives at different paths depending on context:

- **Manual / CLI**: `~/.gardener/backlog.sqlite` — used by `scripts/backlog-db.sh` and manual operations. Override with `GARDENER_DB_PATH` or `--db`.
- **Runtime artifact**: `.cache/gardener/backlog.sqlite` — written by the gardener runtime during startup. Override with `GARDENER_RUNTIME_DB_PATH` (legacy fallback: `GARDENER_DB_PATH`).

## Prerequisites

- Run commands from the repository root.
- For manual database operations, use the backlog database under
  `~/.gardener/backlog.sqlite` by default.
- Confirm `GARDENER_DB_PATH` if you need to target a different database.

## Core entrypoint: `scripts/backlog-db.sh`

The backlog helper script supports two supported operations:

- `list`: show latest active rows with `task_id`, `title`, `priority`, `status`, `source`, `scope_key`.
- `add`: create a manual task.
- `runbook`: print this operations guide in full.

## List backlog entries

- List most recent tasks:
  - `./scripts/backlog-db.sh list`
- List a different database:
  - `./scripts/backlog-db.sh list --db /path/to/backlog.sqlite`

Example:

```bash
GARDENER_DB_PATH=~/.gardener/backlog.sqlite ./scripts/backlog-db.sh list
```

## Create tasks

Use `add` for immediate manual task creation.

```bash
./scripts/backlog-db.sh add \
  --title "GARD-xx: task title" \
  --details "Detailed action in this repo" \
  --priority P1 \
  --scope runtime \
  --kind maintenance \
  --source manual
```

Required fields:

- `--title`
- `--details`

Optional fields:

- `--priority` (`P0|P1|P2`, default `P1`)
- `--scope` (default `runtime`)
- `--kind` (`feature|maintenance|quality_gap|bugfix|infra|merge_conflict|pr_collision`)
- `--status` (`ready|leased|in_progress|merge_pending|complete|failed|unresolved`, default `ready`)
- `--source` (default `manual`)
- `--id` (default `manual:<scope>:auto-<unix_ms>`)
- `--db` (defaults to `~/.gardener/backlog.sqlite`)

## State glossary

Backlog status values are:

- `ready`: waiting to be claimed
- `leased`: worker ownership reservation exists
- `in_progress`: actively being worked
- `merge_pending`: done work waiting for merge queue
- `complete`: finished and recorded
- `failed`: no longer eligible
- `unresolved`: unresolved blocker requiring manual intervention

## Safe operation pattern

1. Prefer additive operations (`list`, `add`) first.
2. Use exact task ids for edits and keep IDs stable when possible.
3. Avoid writing arbitrary SQL unless a recovery action is needed.

## Related documentation

- `.codex/skills/backlog-db/SKILL.md`
- `.claude/skills/backlog-db/SKILL.md`
- `docs/conventions/workflow.md`
