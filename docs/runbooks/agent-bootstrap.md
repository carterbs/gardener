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
- Review canonical validation command usage: [AGENTS.md#validation-commands](../../AGENTS.md#validation-commands)

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

Validation guidance for this bootstrap run:

- Fast (local verification): run the fast tier in `docs/conventions/workflow.md` first for quick signal while iterating.
  - `scripts/doc-gardening.sh`
  - `scripts/check-skills-sync.sh`
  - `scripts/check-no-warnings.sh`
  - `scripts/check-migrations-wired.sh`
  - `scripts/check-binary-blobs.sh`
  - `scripts/run-script-lint-fixture-tests.sh`
- Full (merge / pre-commit equivalence): run `scripts/run-validate.sh` or `./.githooks/pre-commit` once bootstrap quality/backlog checks are stable.

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

## Domain start points

### `tools/gardener/tests` (`verification-harness`)

- Entry files:
  - `tools/gardener/tests/docs_readme.rs`
  - `tools/gardener/tests/cli_smoke.rs`
- Start-here command:
  - `cargo test -p gardener --test docs_readme`

### `scripts/fixtures` (`repository-automation`)

- Entry files:
  - `scripts/fixtures/check-migrations-wired/passing/migrations/001_init.sql`
  - `scripts/run-script-lint-fixture-tests.sh`
- Start-here command:
  - `bash scripts/run-script-lint-fixture-tests.sh`
