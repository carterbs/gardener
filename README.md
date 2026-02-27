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

## Vision

See:

- [Gardener Vision](plans/initial-build/00-gardener-vision.md)
