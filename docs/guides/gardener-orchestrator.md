# Gardener Orchestrator Guide

## Commands

- `npm run gardener:run -- --quit-after 1 --config tools/gardener/tests/fixtures/configs/phase09-cutover.toml`
- `npm run gardener:sync`

## `gardener:sync` Contract

`gardener:sync` executes reconciliation-only behavior:

1. Startup audits/reconciliation (skipped in `execution.test_mode=true` fixtures).
2. PR/worktree/backlog synchronization path.
3. Backlog snapshot export to `.cache/gardener/backlog-snapshot.md`.
4. Exit code `0` on healthy sync; non-zero on typed sync failure.

No worker pool is launched in sync mode.
