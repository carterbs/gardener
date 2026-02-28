# Workflow Conventions

## Termination Modes

- `--prune-only`: prune/reconcile only, then exit.
- `--validate`: run configured validation command and exit with its status.
- `--backlog-only`: startup audits and backlog maintenance without worker pool launch.
- `--quit-after <N>`: run worker pool until `N` task completions are reached, then exit.
- `--sync-only`: reconciliation-only flow with startup audits (when not in test mode), PR/worktree sync, backlog snapshot export, then deterministic exit.

## Quality Grades

Quality-grade document ownership is in Gardener runtime startup audits. External orchestration should delegate to Gardener instead of maintaining a separate grade generation path.
