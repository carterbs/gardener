# Gardener Runtime

Gardener orchestration is Rust-first.

- Runtime entrypoint: `scripts/brad-gardener`
- Run workers: `npm run gardener:run -- --target 1 --config <path>`
- Reconciliation only: `npm run gardener:sync`

Legacy TypeScript orchestration entrypoints are not part of active runtime execution.
