# Gardener

Gardener is a Rust orchestrator that makes real repositories more agent-ready.

It improves repository legibility and reliability so coding agents can execute work with deterministic feedback loops, explicit task state transitions, and auditable run artifacts.

## Why Gardener

Most brownfield repos are hard for autonomous agents to navigate safely:

- Tooling is inconsistent across machines.
- Architecture rules live in docs but not enforcement.
- Quality signals are stale or fragmented.
- Backlogs are not shaped as executable, verifiable tasks.

Gardener addresses this by building and maintaining a deterministic operating layer in-repo.

## What It Does

- Runs startup audits and reconciliation.
- Maintains typed worker execution flows.
- Tracks backlog/task state and snapshots.
- Produces structured logs and quality outputs.
- Supports reconciliation-only sync mode.

## Install

From this repo:

```bash
cargo install --path tools/gardener
```

Then verify:

```bash
gardener --help
```

## Quick Start

1. Run one-time interactive triage:

```bash
gardener --triage-only --config tools/gardener/tests/fixtures/configs/phase10-full.toml
```

2. Run bounded execution:

```bash
gardener --quit-after 1 --config tools/gardener/tests/fixtures/configs/phase10-full.toml
```

## Validation

```bash
cargo test -p gardener --all-targets
cargo llvm-cov -p gardener --all-targets --summary-only
```

Coverage gate helper:

```bash
# Enforces 90% minimum line coverage by default for Gardener coverage runs.
./scripts/test-gardener-coverage.sh

# Override the minimum at runtime:
COVERAGE_MIN_LINE=95 ./scripts/test-gardener-coverage.sh

# Override ignored source files (optional):
COVERAGE_IGNORE_REGEX="/tools/gardener/src/(agent/mod\.rs|agent/factory\.rs)" \
  ./scripts/test-gardener-coverage.sh

# Override the ignore manifest (optional, reviewed list format):
COVERAGE_IGNORE_MANIFEST=./scripts/coverage-ignore-manifest.txt ./scripts/test-gardener-coverage.sh
```

### Validation pipeline

The canonical validation workflow is documented in:

- [Validation and pre-commit flow](docs/conventions/workflow.md#validation-and-pre-commit-flow)


## Git Hooks

Enable the local pre-commit hook to run Gardener validation before each commit:

```bash
git config core.hooksPath .githooks
```

or run the repo helper script:

```bash
./scripts/setup-git-hooks.sh
```

Pre-commit now executes:

```text
./.githooks/pre-commit
```

That means each commit runs:

1. auto-format on staged Rust files
2. the full `scripts/run-validate.sh` pipeline above

### Pre-commit remediation playbook

Use the canonical playbook in the workflow docs:

- [Validation and pre-commit flow](docs/conventions/workflow.md#validation-and-pre-commit-flow)

## Vision

See:

- [Gardener Vision](plans/initial-build/00-gardener-vision.md)
