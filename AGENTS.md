# Gardener Runtime

Gardener orchestration is Rust-first.

- Runtime entrypoint: `scripts/brad-gardener`
- Run workers: `npm run gardener:run -- --quit-after 1 --config <path>`
- Reconciliation only: `npm run gardener:sync`

Legacy TypeScript orchestration entrypoints are not part of active runtime execution.

## Commit policy

- All commits MUST pass pre-commit hooks. `git commit --no-verify` is not allowed.
- If pre-commit fails, fix the underlying issue and commit a real fix; do not bypass or mask failures.
