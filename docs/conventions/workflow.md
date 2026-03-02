# Workflow Conventions

## Termination Modes

- `--prune-only`: prune/reconcile only, then exit.
- `--validate`: run configured validation command and exit with its status.
- `--backlog-only`: startup audits and backlog maintenance without worker pool launch.
- `--quit-after <N>`: run worker pool until `N` task completions are reached, then exit.
- `--sync-only`: reconciliation-only flow with startup audits (when not in test mode), PR/worktree sync, backlog snapshot export, then deterministic exit.
- `seed-backlog --mode dry-run|write`: run the seeding phase as a standalone binary (default `dry-run` prints recommended tasks to stdout without backlog writes).

## Agent mode-selection playbook

- Use `--agent` to select the runtime backend for this invocation (`claude` or `codex`).
- Use `--worker-mode` to override how workers execute:
  - `normal` (default): run the normal worker finite-state machine path.
  - `stub_complete`: claim-and-complete fast path for scheduler-oriented validation runs.
- For CI-style validation where you want deterministic completion and minimal side effects, combine:
  - `--target N` for bounded completion,
  - `--sync-only` for startup-only reconciliation,
  - `--backlog-only` for maintenance passes.

## Quality Grades

Quality-grade document ownership is in Gardener runtime startup audits. External orchestration should delegate to Gardener instead of maintaining a separate grade generation path.
