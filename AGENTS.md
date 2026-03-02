# Gardener Runtime

Gardener orchestration is Rust-first.

- Runtime entrypoint: `cargo run -p gardener --bin gardener --`
- Run workers: `cargo run -p gardener --bin gardener -- --quit-after 1 --config <path>`
- Reconciliation only: `cargo run -p gardener --bin gardener -- --sync-only --config <path>`

Legacy TypeScript orchestration entrypoints are not part of active runtime execution.

## Commit policy

- All commits MUST pass pre-commit hooks. `git commit --no-verify` is not allowed.
- If pre-commit fails, fix the underlying issue and commit a real fix; do not bypass or mask failures.

## Worktree policy

- Use a git worktree for development and testing. Avoid making direct edits in the repository root working copy.
