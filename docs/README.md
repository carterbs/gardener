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

## Fifth: repository map

- [Repository map](./repository-map.md)

## Before editing

1. Read [`AGENTS.md`](../AGENTS.md) and the [root README](../README.md).
2. Read the relevant docs in this index.
3. Use this repository as your source of truth and verify behavior with tests before committing.
