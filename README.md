# Gardener

Gardener is a Rust orchestrator that makes real repositories agent-ready. Point it at a brownfield codebase and it systematically builds the scaffolding — quality grades, prioritized backlogs, steering docs, coverage gates — that lets AI coding agents execute work autonomously.

It doesn't ask you to rewrite your repo first. It meets the codebase where it is, assesses the gap, and starts closing it.

## The Problem

Most repositories weren't designed for autonomous agents. They have ad-hoc scripts that behave differently across machines, architecture rules that live in someone's head, test suites that pass locally and fail in CI for mysterious reasons, and backlogs that are just a vague sense of unease.

Humans navigate this fine. Agents can't. From an agent's perspective, anything it can't find in its context doesn't exist. Ambiguity isn't a speedbump — it's a wall.

## What Gardener Does

Gardener runs as a persistent orchestrator with a live terminal dashboard. A single invocation handles the full lifecycle:

### 1. Triage — Understand the Repo

On first run, Gardener performs an agent-driven discovery pass: scanning for steering documents, architecture docs, CI configuration, test infrastructure, coverage gates, and custom linters. It forms hypotheses about where the repo stands, then runs a short interactive interview to surface what the file scan can't discover on its own.

The result is a **Repo Intelligence Profile** — a versioned document that maps the repo's current state against five agent-readiness dimensions:

- **Agent steering** — quality of AGENTS.md / CLAUDE.md instructions
- **Knowledge accessibility** — docs, architecture references, domain boundaries
- **Mechanical guardrails** — CI, pre-commit hooks, linting, formatting
- **Local feedback loop** — test and validation commands that actually work
- **Coverage signal** — test coverage gates enforced in CI

### 2. Quality Grading — Score Every Domain

Gardener discovers the codebase's domains (not hardcoded — it reads the actual code), then grades each one against a consistent rubric. The quality report covers:

- Per-domain test coverage, test quality (assertion density), and risk exposure
- Repo-wide readiness scores across all five dimensions
- Structural deficiencies ranked by severity
- A primary gap — the single most impactful improvement for agent readiness

Grades use a 9-level scale (A through F with +/-). The grading system uses a hybrid architecture: deterministic tools collect hard evidence (file trees, test counts, coverage artifacts, debt markers, documentation presence), an LLM agent interprets the evidence and produces structured scores, and a deterministic formula converts scores to letter grades. When no LLM is available, a conservative deterministic fallback produces valid (if less nuanced) results.

The system works on any repo given only a path — no setup, no config, no prior knowledge. It supports Rust, TypeScript/JavaScript, Swift, Python, and Go out of the box, and reports unrecognized languages so nothing is invisible.

### 3. Backlog Seeding — Generate Real Tasks

Quality gaps don't just get reported — they become actionable backlog tasks. Gardener seeds a prioritized SQLite-backed task queue with specific, scoped work items derived from the quality assessment:

- Each task has a kind (chore, feature, bugfix, refactor, infra), priority (P0/P1/P2), and clear scope
- Tasks are deduplicated by domain + category — no proliferation of near-duplicates
- The operator reviews and approves seeded tasks before any work begins

### 4. Worker Execution — Do the Work

Gardener spawns parallel workers, each moving a task through an explicit state machine:

```
Understand → Plan → Do → Git → Review → Merge
```

Every step is typed and validated. Workers don't freestyle through ambiguous state — they follow a protocol with clear failure modes and recovery paths. Each task gets its own git worktree and branch, so workers never interfere with each other or with your working copy.

Workers use pluggable agent backends (Claude Code and OpenAI Codex are supported). The review phase runs automated code review with up to 3 revision loops before a PR is opened. Failed tasks are marked and retried with context from the failure.

### 5. Learning — Get Better Over Time

After each task completes or fails, Gardener captures what happened. Post-merge analysis and failure postmortems produce knowledge entries that feed into subsequent prompts — not as vague instruction inflation, but as structured evidence that influences how future tasks are approached.

## The Dashboard

Gardener renders a live TUI (terminal UI) that shows everything at a glance:

- Worker status: which task each worker is on, what FSM state it's in, last tool call, heartbeat age
- Queue stats: ready / active / failed / unresolved / merge-pending counts by priority
- Backlog: upcoming tasks in priority order

Hotkeys let you interact during a run: scroll through workers, view the quality report, retry stuck leases, or quit gracefully.

<!-- TODO: Add TUI screenshot -->

## Install

```bash
cargo install --path tools/gardener
```

Verify:

```bash
gardener --help
```

## Quick Start

### First run — triage your repo

```bash
gardener --triage-only
```

This runs the interactive triage interview and produces the repo intelligence profile. You only need to do this once (Gardener re-triages automatically when the repo drifts significantly).

### See what Gardener would recommend

```bash
gardener --backlog-only --seed-dry-run
```

Prints recommended backlog tasks without writing anything.

### Run one task end-to-end

```bash
gardener --quit-after 1
```

Gardener triages (if needed), seeds the backlog, picks the highest-priority ready task, and executes it through the full understand → plan → do → git → review → merge cycle.

### Run with parallelism

```bash
gardener --quit-after 5 --num-workers 3
```

Three workers execute tasks in parallel, each in its own worktree.

### Quality report only

```bash
gardener --quality-grades-only
```

Regenerates the quality grade document without running any tasks.

## Configuration

Gardener works with zero configuration — sensible defaults apply. For customization, create a `gardener.toml` at your repo root:

```toml
[orchestrator]
parallelism = 3              # parallel workers

[agent]
default = "claude"           # or "codex"

[validation]
command = "cargo test"       # your repo's validation command

[seeding]
backend = "claude"
model = "sonnet"
```

See [the full config reference](docs/conventions/workflow.md) for all options.

## How It Fits Together

```
gardener
  ├── Triage          → repo-intelligence.toml (readiness profile)
  ├── Quality Grades  → docs/quality-grades.md (per-domain scores)
  ├── Backlog Seed    → backlog.sqlite (prioritized task queue)
  └── Worker Pool     → git worktrees, PRs, merged code
       ├── Worker 1: Understand → Plan → Do → Git → Review → Merge
       ├── Worker 2: ...
       └── Worker 3: ...
```

Each run is auditable. Structured OTEL-format logs capture every event, tool call, and state transition in `.gardener/otel-logs.jsonl`.

## Standalone Tools

Individual phases can be run independently for debugging or integration:

| Binary | Purpose |
|---|---|
| `seed-backlog` | Run backlog seeding standalone |
| `understand` | Run the understand phase for a task |
| `plan` | Run the planning phase for a task |
| `do-task` | Run the doing phase for a task |
| `git-push` | Run the git/push phase for a task |
| `review-pr` | Run automated review for a task |
| `merge-pr` | Run the merge phase for a task |
| `friction-analysis` | Analyze repo friction points |
| `otel-logs` | Inspect structured run logs |

## Development

```bash
# Run tests
cargo test -p gardener --all-targets

# Coverage (enforces 90% minimum)
./scripts/test-gardener-coverage.sh

# Enable pre-commit hooks
./scripts/setup-git-hooks.sh
```

The canonical validation workflow is documented in [docs/conventions/workflow.md](docs/conventions/workflow.md).

## The Outcome

After Gardener runs, the repository is a different kind of place. An agent dropped into it cold can read the steering docs, follow the pointers, understand the domain structure, run the validation command, and trust what it gets back. It can pick a task from the backlog and know that the task is real, sized right, and unambiguous about success criteria.

The humans who operate the repository stop being the load-bearing wall. They're steering. The agents are executing.
