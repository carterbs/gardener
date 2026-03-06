# Gardener Runtime

Gardener orchestration is Rust-first.

- Runtime entrypoint: `cargo run -p gardener --bin gardener --`
- Run workers: `cargo run -p gardener --bin gardener -- --quit-after 1 --config <path>`
- Reconciliation only: `cargo run -p gardener --bin gardener -- --sync-only --config <path>`

## Validation commands

Use this matrix as the default validation path for agent work and human review.

| scenario | command | when to run |
|---|---|---|
| Build check | `cargo build -p gardener --all-targets` | Verify the crate compiles after code changes before test execution. |
| Targeted tests | `cargo test -p gardener --all-targets -- <test_name_or_filter>` | Run focused validation for a crate, module, or test filter. |
| Full validation | `./scripts/run-validate.sh` | Run the canonical local quality gate sequence before review or handoff. |
| Migration checks | `./scripts/check-migrations-wired.sh` | Run after adding/removing/renaming migration files or schema-related code. |
| Commit parity | `./.githooks/pre-commit` | Mirror commit-time checks locally before creating a commit. |

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
