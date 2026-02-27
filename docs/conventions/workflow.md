# Workflow Conventions

## Termination Modes

- `--prune-only`: prune/reconcile only, then exit.
- `--backlog-only`: startup audits and backlog maintenance without worker pool launch.
- `--target <N>`: run worker pool until target completions reached.
- `--sync-only`: reconciliation-only flow with startup audits (when not in test mode), PR/worktree sync, backlog snapshot export, then deterministic exit.

## Quality Grades

Quality-grade document ownership is in Gardener runtime startup audits. External orchestration should delegate to Gardener instead of maintaining a separate grade generation path.
