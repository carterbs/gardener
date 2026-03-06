# Gardener Runtime

Gardener orchestration is Rust-first.

- Runtime entrypoint: `cargo run -p gardener --bin gardener --`
- Run workers: `cargo run -p gardener --bin gardener -- --quit-after 1 --config <path>`
- Reconciliation only: `cargo run -p gardener --bin gardener -- --sync-only --config <path>`

Legacy TypeScript orchestration entrypoints are not part of active runtime execution.

## Commit policy

- All commits MUST pass pre-commit hooks. `git commit --no-verify` is not allowed.
- If pre-commit fails, fix the underlying issue and commit a real fix; do not bypass or mask failures.

## Architecture map

- `tools/gardener/src/` — runtime core and phase orchestration (`git_phase`, `seed_runner`, protocol/worker lifecycle glue).
- `tools/gardener/src/*_phase.rs` — phase-by-phase execution handlers used by reconciliations and worker orchestration.
- `tools/gardener/tests/` — harnesses and fixtures for integration behavior (`cargo test -p gardener --test ...`), including run/phase contracts.
- `tools/gardener/migrations/` — persistence schema evolution and backfill assumptions for task/backlog state.
- `docs/quality-grades/` (including generated grading output docs/quality-grades.md artifacts) — quality rubric and grading result expectations.
- `.gardener/otel-logs.jsonl` — emitted telemetry stream for worker/run diagnostics and post-failure analysis.

## Worktree policy

- Use a git worktree for development and testing. Avoid making direct edits in the repository root working copy.
