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

## Validation and pre-commit flow

- Gardener validation command entrypoint is configured under `[validation] command` and `[startup] validation_command` in `gardener.toml`.
- `scripts/run-validate.sh` is the canonical project validation command.
  - It executes each custom linter in order:
    - `scripts/doc-gardening.sh`
    - `scripts/check-skills-sync.sh`
    - `scripts/check-no-warnings.sh`
    - `scripts/check-migrations-wired.sh`
    - `scripts/check-binary-blobs.sh`
    - `scripts/run-script-lint-fixture-tests.sh`
  - It ensures `cargo-llvm-cov` is available before running:
    - `./scripts/test-gardener-coverage.sh`
  - It fails fast and exits on first failed stage.
- The `.githooks/pre-commit` hook runs this command sequence:
  - auto-format staged Rust files with `rustfmt`
  - re-add formatted Rust files to the index
  - run `scripts/run-validate.sh`
- Git hooks path is set by `./scripts/setup-git-hooks.sh` (`core.hooksPath=.githooks`).

Pre-commit remediation playbook:

1. Reproduce the exact pre-commit path:

```bash
./.githooks/pre-commit
```

2. Capture the first failing step from the hook output (for example:
   `scripts/check-skills-sync.sh`, `scripts/check-no-warnings.sh`, `scripts/test-gardener-coverage.sh`).

3. Fix the underlying issue and rerun `./.githooks/pre-commit`:

- `skills` check mismatch: copy files from the command suggestions in `scripts/check-skills-sync.sh` output.
- clippy warnings: address the warning text, then re-run `./scripts/check-no-warnings.sh`.
- doc-gardening maintenance issues: review and fix failing checks from `./scripts/doc-gardening.sh`.
- migration wiring failures: add each missing migration include to `tools/gardener/src/backlog_store.rs`.
- binary blob failures: remove blocked files listed by `scripts/check-binary-blobs.sh` from the commit or move them outside git history.
- fixture-script failures: rerun `./scripts/run-script-lint-fixture-tests.sh` after updating script docs or fixtures.
- coverage/test failures: inspect `./scripts/test-gardener-coverage.sh` output and harden the relevant code paths.

4. Re-stage updates (`git add`), then retry commit.
