# Repository Map

This repository is organized into a small number of stable file families to keep agent navigation predictable.

## Family 1: Orchestration Runtime

- `tools/gardener/src/`
  - Core orchestration, runtime state machine, and quality evidence logic.
- `tools/gardener/src/bin/`
  - CLI entrypoints (`gardener`, `seed-backlog`, `review-pr`, etc.).
- `tools/gardener/tests/`
  - Integration and unit tests for orchestration behavior.
- `tools/gardener/migrations/`
  - Database migration SQL for backlog state.
- `tools/gardener/mockups/`
  - Design notes and UI mockups.

## Family 2: Execution Entry and Utilities

- `scripts/`
  - Runtime entrypoint (`cargo run -p gardener --bin gardener --`), validation hooks, and developer tooling.
- `scripts/*`
  - Repository support scripts used by CI, checks, and local automation.

## Family 3: Governance and Contracts

- `AGENTS.md`
  - Runtime and development constraints.
- `CLAUDE.md`
  - Agent compatibility notes.
- `.codex/skills/`
  - Codex skills used by agents during workflows.
- `thoughts/`
  - Plans, analysis, and operational notes.
- `plans/`
  - Execution plans and historical project context.

## Family 4: Reference and Knowledge

- `docs/`
  - Canonical index and knowledge for navigation and agent readiness.
- `docs/conventions/`
  - Workflow and behavioral conventions.
- `docs/references/`
  - Long-form rationale and design references.

## Family 5: Root configuration and metadata

- `Cargo.toml`, `Cargo.lock`
  - Workspace and dependency state.
- `gardener.toml`
  - Primary runtime configuration default.
- `.github/`
  - CI workflows and project automation contracts.
- `.githooks/`
  - Git hook integration.

## How to choose a file family first

1. Start with `docs/README.md` and this map for context.
2. Read `AGENTS.md` and `README.md` for execution constraints.
3. Use `tools/gardener/` for runtime/test implementation changes.
4. Use `scripts/` for command behavior and automation changes.
