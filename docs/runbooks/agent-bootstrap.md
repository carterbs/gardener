# Agent Bootstrap Runbook for First-Run Worktree Setup

## Scope

This runbook is used when an agent enters a newly created or freshly checked-out
git worktree and needs to make Gardener runnable end-to-end.

## Prerequisites

- Start from the repository root of the worktree.
- Confirm required tooling is available (`git`, `cargo`, and `bash`).
- Read the navigation files first:
  - `AGENTS.md`
  - `CLAUDE.md`
  - `docs/README.md`

## Bootstrap sequence

1. Verify the worktree is isolated and on the expected branch.

```bash
git worktree list
git status --short
```

2. Ensure repository hooks are installed so commit-time validation matches CI behavior.

```bash
./scripts/setup-git-hooks.sh
```

3. Run startup quality and backlog-seeding checks against the local config.

```bash
cargo run -p gardener --bin gardener -- --quality-grades-only --config gardener.toml
cargo run -p gardener --bin gardener -- --backlog-only --config gardener.toml
```

4. Run a bounded worker warmup to confirm end-to-end startup to completion.

```bash
cargo run -p gardener --bin gardener -- --quit-after 1 --config gardener.toml
```

5. Inspect first-run artifacts.

- `docs/quality-grades.md` reflects startup quality state.
- `.gardener/otel-logs.jsonl` contains startup and first-task lifecycle events.
- `~/.gardener/backlog.sqlite` has seeded tasks from startup (`cargo run -p gardener --bin backlog-db -- list` to confirm).
- `docs/runbooks/backlog-operations.md` for manual interventions when required.

6. Optional startup reconciliation pass (no long-running workers).

```bash
cargo run -p gardener --bin gardener -- --sync-only --config gardener.toml
```

## Recovery and escalation

If warmup fails, capture evidence in this order before retry:

1. Rerun with startup diagnostics:

```bash
scripts/startup-diagnostics.sh --run-id "<run_id>" --log-path ".gardener/otel-logs.jsonl" --output ".gardener/startup-diagnostics.jsonl" --error "first-run bootstrap failed"
```

2. Re-check the failure timeline and work from the logs:

```bash
$LOG_QUERY_BIN timeline --run-id "<run_id>"
```

3. Triage runtime details with:

`docs/runtime-failure-otel-jsonl-cookbook.md`

## Completion criteria

- Startup grades generated and readable.
- Backlog can be listed without DB write failures.
- One bounded worker run completes cleanly and exits normally.
