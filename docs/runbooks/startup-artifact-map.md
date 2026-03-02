# Startup Artifact Map Runbook for Agent Steering

This runbook helps agents align startup evidence with the `agent_steering` quality dimension.

## Scope

Use this runbook when startup indicates:

- Startup health is degraded (`agent_steering` appears in quality output),
- onboarding or triage work is still blocked,
- automated startup checks are failing and you need to prove why.

## Startup artifact map

| Artifact | Location | Why it exists | Typical steering questions |
| --- | --- | --- | --- |
| Repo intelligence profile | `.gardener/repo-intelligence.toml` (or configured `triage.output_path`) | Discovery and interview output captured by startup | Does this repository expose clear steering docs for Codex/Claude and stable conventions? |
| Quality-grade summary | `docs/quality-grades.md` (or configured `quality_report.path`) | Startup writes the latest readiness document before worker startup | Is `agent_steering` the lowest dimension or the current `primary_gap`? |
| Startup logs | `.gardener/otel-logs.jsonl` (or `GARDENER_LOG_PATH`) | Structured startup audit events and errors | Did startup run triage/profile load and complete quality refresh? |
| Startup diagnostics | `scripts/startup-diagnostics.sh` output (if startup fails) | One-command summary for failure triage | Which startup audit step failed first and what was the exact error? |
| Backlog seed evidence | `.cache/gardener/backlog.sqlite` | Startup may upsert tasks mapped from startup findings | Did startup seed tasks for `agent_steering` gaps that need manual routing? |
| Startup backup copy | `.cache/gardener/backlog.sqlite.bak` and sidecars | Preserves DB state when startup runs | Do startup errors correlate with missing or stale backlog state? |

## Quick startup evidence collection

- Gather startup-only artifacts:

```bash
scripts/brad-gardener --quality-grades-only --config <path/to/gardener.toml>
```

- Re-run full startup flow for startup-seeding diagnostics:

```bash
scripts/brad-gardener --backlog-only --config <path/to/gardener.toml>
```

- Emit startup diagnostic bundle on failure (automatic in runtime):

```bash
scripts/startup-diagnostics.sh --run-id <run-id> --log-path <log-path> --output <file> --error <message>
```

## Steering-specific interpretation

- If `agent_steering` is weak in the generated `repo-intelligence`:
  - verify steering docs referenced in `AGENTS.md`, `CLAUDE.md`, and `.codex/.claude` skill surfaces,
  - keep startup quality artifacts updated after each clarification pass,
  - seed or create follow-up tasks that explicitly improve steering guidance.
- If startup fails before worker handoff:
  - read the latest `startup.*` events in logs,
  - check missing profile message and ensure triage output path exists,
  - compare `repo-intelligence.toml` and quality report paths to the configured scope (`--working-dir`).

