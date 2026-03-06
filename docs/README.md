# Agent Navigation Index

This directory is the canonical index for agent-oriented repository navigation. Start here before deeper exploration.

## First: repository constraints

- [`AGENTS.md`](../AGENTS.md)
- [`README.md`](../README.md)

## Second: execution and workflow

- [Workflow conventions](./conventions/workflow.md)
- [Recurring doc-gardening maintenance](../scripts/doc-gardening.sh)
- [Repository reference essay](./references/codex-agent-team-article.md)
- [Triage and worktree workflow docs](./conventions/workflow.md)
- [OTEL JSONL runtime failure triage cookbook](./runtime-failure-otel-jsonl-cookbook.md)

## Third: reusable agent capabilities

- [Backlog DB skill](../.codex/skills/backlog-db/SKILL.md)
- [Debugging logs skill](../.codex/skills/log-debugging/SKILL.md)
- [Session replay skill](../.codex/skills/session-replay/SKILL.md)

## Fourth: agent runbooks

- [Backlog operations runbook](./runbooks/backlog-operations.md)
- [Agent bootstrap runbook (first run)](./runbooks/agent-bootstrap.md)
- [Startup artifact map runbook](./runbooks/startup-artifact-map.md)
- [Lima isolated runtime runbook](./runbooks/lima-isolated-runtime.md)

## Domain start points

### `tools/gardener/tests` (verification-harness)

- Entry files
  - [`tools/gardener/tests/docs_readme.rs`](../tools/gardener/tests/docs_readme.rs)
  - [`tools/gardener/tests/cli_smoke.rs`](../tools/gardener/tests/cli_smoke.rs)
- Start here command
  - `cargo test -p gardener --test docs_readme`

### `scripts/fixtures` (repository-automation)

- Entry files
  - [`scripts/fixtures/check-migrations-wired/passing/migrations/001_init.sql`](../scripts/fixtures/check-migrations-wired/passing/migrations/001_init.sql)
  - [`scripts/run-script-lint-fixture-tests.sh`](../scripts/run-script-lint-fixture-tests.sh)
- Start here command
  - `bash scripts/run-script-lint-fixture-tests.sh`

## Fifth: repository map

- [Repository map](./repository-map.md)

These sections close the verification-harness and repository-automation start-point gaps noted in [quality steering evidence](./quality-grades/agent_steering.md).

## Before editing

1. Read [`AGENTS.md`](../AGENTS.md) and the [root README](../README.md).
2. Read the relevant docs in this index.
3. Use this repository as your source of truth and verify behavior with tests before committing.
