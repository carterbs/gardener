# Lima Bootstrap + Auth + Agent CLI Setup Plan

## Overview

Create a single bootstrap workflow that provisions a Lima VM for Gardener, installs all required runtime dependencies, configures GitHub auth for push/PR, installs both agent CLIs (`codex`, `claude`), verifies compatibility with Gardener's adapter expectations, and provides a repeatable non-interactive preflight for overnight runs.

## Current State Analysis

- Gardener currently expects `codex` and `claude` binaries to be available on `PATH` and probes each with `--help` and `--version` before execution ([codex.rs](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/gardener/src/agent/codex.rs:25), [codex.rs](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/gardener/src/agent/codex.rs:31), [claude.rs](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/gardener/src/agent/claude.rs:25), [claude.rs](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/gardener/src/agent/claude.rs:31)).
- Codex adapter execution uses `codex exec --json ... --model ... -C <cwd> -o <file>` and optionally `--output-schema` ([codex.rs](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/gardener/src/agent/codex.rs:99)).
- Claude adapter execution uses `claude -p ... --output-format stream-json --verbose --model ...` with optional `--max-turns` and `--dangerously-skip-permissions` in permissive mode ([claude.rs](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/gardener/src/agent/claude.rs:90)).
- Runtime config is mixed-backend by phase (Codex + Claude), so both CLIs must be installed for default `gardener.toml` execution ([gardener.toml](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/gardener.toml:17)).
- A minimal Lima profile and helper script exist, but they currently provision base OS deps + Rust only; they do not install Node, Codex CLI, Claude CLI, or perform auth/preflight checks ([gardener-isolated.yaml.tmpl](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/lima/gardener-isolated.yaml.tmpl:29), [lima-gardener.sh](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/scripts/lima-gardener.sh:45), [lima-isolated-runtime.md](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/docs/runbooks/lima-isolated-runtime.md:25)).

## Desired End State

- One command to bootstrap VM and toolchain.
- One command to run an interactive auth wizard for required credentials (GitHub + agents).
- One command to run strict preflight checks before overnight execution.
- One command to run Gardener safely in the VM with predictable env.
- Clear failure messages and remediation guidance for every prerequisite.

## Key Discoveries

- Official Claude Code install path is npm global package (`@anthropic-ai/claude-code`) and login is interactive via CLI start/auth flow (Anthropic docs: https://docs.anthropic.com/en/docs/claude-code/getting-started, https://docs.anthropic.com/en/docs/claude-code/quickstart).
- Official Codex CLI install path is npm global package (`@openai/codex`) and login can use ChatGPT sign-in or API key path (OpenAI primary docs: https://github.com/openai/codex, https://help.openai.com/en/articles/11096431).
- Gardener uses CLI capability probing, so we can fail early with deterministic checks rather than allowing mid-run worker failures.

## What We're NOT Doing

- No changes to Gardener adapter internals in this phase.
- No migration to Docker profile in parallel.
- No host-side credential sharing (`~/.ssh`, host keychain, host GH auth store).
- No attempt to produce macOS-runnable binaries from inside Linux VM.

## Implementation Approach

Build a small control plane around the existing `scripts/lima-gardener.sh` entrypoint:

1. Harden provisioning in the Lima template (add Node/npm + tmux + minimal utilities).
2. Add a VM-internal bootstrap script that is idempotent and version-checking.
3. Add wrapper subcommands in `scripts/lima-gardener.sh` for `bootstrap`, `auth-all`, `preflight`, and `overnight`.
4. Document a deterministic operator sequence in the runbook with exact commands.

## Implementation Phases

### Phase 1: Provisioning Baseline and Script Surface

Overview:
Expand base VM provisioning and CLI surface so downstream install/auth is deterministic.

Changes required:
- Update [tools/lima/gardener-isolated.yaml.tmpl](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/lima/gardener-isolated.yaml.tmpl) to include:
  - OS deps already present + `nodejs`, `npm`, `tmux`, `ripgrep`.
  - Keep repo-only mount, no extra host mounts.
- Extend [scripts/lima-gardener.sh](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/scripts/lima-gardener.sh):
  - New commands: `bootstrap`, `auth-all`, `preflight`, `overnight`.
  - A generic `vm_exec` helper for consistent shell execution.

Success criteria:
- `./scripts/lima-gardener.sh up` creates/starts VM.
- `./scripts/lima-gardener.sh bootstrap` runs without manual edits.
- `./scripts/lima-gardener.sh status` reliably reports instance state.

Confirmation gate:
- Run `up`, `bootstrap`, then re-run `bootstrap` to verify idempotence.

### Phase 2: Tool Install + Auth Workflow

Overview:
Install and verify GitHub, Codex, and Claude in the VM with explicit auth paths.

Changes required:
- Add a new script: `scripts/lima-bootstrap-vm.sh` (executed inside VM), responsible for:
  - Verifying `cargo`, `rustup`, `node`, `npm`, `git`, `gh`.
  - Installing `@openai/codex` and `@anthropic-ai/claude-code` via npm (or upgrading if already installed).
  - Verifying commands required by adapters:
    - `codex --help`, `codex --version`.
    - `claude --help`, `claude --version`.
  - Optionally pinning minimum versions to avoid CLI-flag drift.
- Add `auth-all` command in `scripts/lima-gardener.sh`:
  - `gh auth login` (required for push/PR).
  - `codex login` or `codex` interactive sign-in path.
  - `claude` interactive auth flow.
  - Final status checks: `gh auth status`, agent auth smoke checks.

Success criteria:
- `codex` and `claude` are installed and runnable in VM.
- `gh auth status` succeeds.
- Auth commands complete without touching host credential stores.

Confirmation gate:
- Execute an in-VM test push to a throwaway branch and optionally `gh pr create --draft`.

### Phase 3: Gardener Compatibility Preflight

Overview:
Add strict readiness checks matching actual Gardener runtime requirements before overnight runs.

Changes required:
- Add `preflight` command in `scripts/lima-gardener.sh` that checks:
  - Working directory mounted and writable (`/workspace/gardener`).
  - `git worktree` operations function in mounted repo.
  - `cargo run -p gardener --bin gardener -- --help` works.
  - Adapter prerequisites pass (via Gardener startup checks or direct CLI probes).
  - Required models and backend settings resolve from `gardener.toml`.
- Optional: a small script `scripts/lima-preflight.sh` executed in VM for cleaner output formatting.

Success criteria:
- One command reports pass/fail with actionable remediation.
- Preflight fails fast if any required dependency/auth is missing.

Confirmation gate:
- Run preflight in both success and forced-failure scenarios (e.g., temporarily hide `codex` from `PATH`).

### Phase 4: Overnight Runtime UX + Documentation

Overview:
Make unattended runs operationally safe with tmux and explicit startup/reattach commands.

Changes required:
- Add `overnight` command in `scripts/lima-gardener.sh`:
  - Starts `tmux` session if missing.
  - Runs Gardener command inside tmux with passed args.
  - Prints attach/detach instructions and log locations.
- Update [docs/runbooks/lima-isolated-runtime.md](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/docs/runbooks/lima-isolated-runtime.md):
  - Add end-to-end sequence: `up -> bootstrap -> auth-all -> preflight -> overnight`.
  - Add troubleshooting matrix for auth/login/install failures.
  - Add cleanup and token revocation procedures.

Success criteria:
- Operator can start run, disconnect terminal, and reattach later.
- Runbook includes exact recovery commands for common failures.

Confirmation gate:
- Manual dry-run of overnight flow with reconnect.

## Script Contract (Target UX)

From repo root:

```bash
./scripts/lima-gardener.sh up
./scripts/lima-gardener.sh bootstrap
./scripts/lima-gardener.sh auth-all
./scripts/lima-gardener.sh preflight
./scripts/lima-gardener.sh overnight -- --config gardener.toml --quit-after 5
```

Supporting commands:

```bash
./scripts/lima-gardener.sh shell
./scripts/lima-gardener.sh run -- --sync-only --config gardener.toml
./scripts/lima-gardener.sh stop
./scripts/lima-gardener.sh delete
```

## Testing Strategy

Automated:
- `bash -n scripts/lima-gardener.sh scripts/lima-bootstrap-vm.sh scripts/lima-preflight.sh`
- `shellcheck` (if available) for all new scripts.
- Basic command-level smoke tests for helper subcommands with a running instance.

Manual:
- Fresh machine bootstrap path from zero state.
- Auth path validation for `gh`, `codex`, `claude`.
- One bounded Gardener run (`--quit-after 1`) inside VM.
- Overnight tmux run with host terminal disconnect/reattach.

## Risks and Mitigations

- CLI auth UX drift (Codex/Claude updates):
  - Mitigation: detect failures and print up-to-date remediation links.
- Package-manager drift across Ubuntu images:
  - Mitigation: keep apt list minimal and idempotent; fail with explicit missing package output.
- Token scope overreach:
  - Mitigation: document minimum scopes and recommend revocation workflow.

## References

- Gardener adapters:
  - [tools/gardener/src/agent/codex.rs](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/gardener/src/agent/codex.rs:25)
  - [tools/gardener/src/agent/claude.rs](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/gardener/src/agent/claude.rs:25)
- Runtime config:
  - [gardener.toml](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/gardener.toml:14)
- Current Lima artifacts:
  - [tools/lima/gardener-isolated.yaml.tmpl](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/tools/lima/gardener-isolated.yaml.tmpl:1)
  - [scripts/lima-gardener.sh](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/scripts/lima-gardener.sh:1)
  - [docs/runbooks/lima-isolated-runtime.md](/Users/bradcarter/Documents/Dev/gardener/.worktrees/lima-agent-profile/docs/runbooks/lima-isolated-runtime.md:1)
- External primary docs:
  - Anthropic Claude Code setup: https://docs.anthropic.com/en/docs/claude-code/getting-started
  - Anthropic Claude Code quickstart: https://docs.anthropic.com/en/docs/claude-code/quickstart
  - OpenAI Codex CLI repo: https://github.com/openai/codex
  - OpenAI Codex CLI help article: https://help.openai.com/en/articles/11096431
