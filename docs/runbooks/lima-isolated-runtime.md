# Lima Isolated Runtime Runbook

## Goal

Run Gardener overnight with host file exposure narrowed to the repository only, while still allowing commit/push/PR workflows.

## Security posture

- Gardener runs inside a Lima Linux VM instance.
- Host mount scope is one writable path: the repository root.
- No host home-directory mount (no direct `~/Downloads`, `~/.ssh`, `~/Documents`, etc.).
- Git pushes and PR creation happen via `gh auth login` inside the VM.

## Files

- Lima template: `tools/lima/gardener-isolated.yaml.tmpl`
- Helper script: `scripts/lima-gardener.sh`

## Prerequisites

- macOS with Lima installed (`brew install lima`).
- Docker is not required for this path.
- Run from repository root.

## First-time setup

1. Start and provision the VM.

```bash
./scripts/lima-gardener.sh up
```

2. Authenticate GitHub in the VM (for push/PR as your account).

```bash
./scripts/lima-gardener.sh auth
```

3. Set git identity in the VM shell.

```bash
./scripts/lima-gardener.sh shell
# inside VM
git config --global user.name "Your Name"
git config --global user.email "you@example.com"
exit
```

## Run Gardener in VM

Interactive TUI:

```bash
./scripts/lima-gardener.sh run
```

Bounded worker execution:

```bash
./scripts/lima-gardener.sh run --quit-after 1 --config gardener.toml
```

Reconciliation-only:

```bash
./scripts/lima-gardener.sh run --sync-only --config gardener.toml
```

## Worktree compatibility

Gardener creates worker worktrees under the mounted repository. Because the full repo path is mounted read/write, the native worktree lifecycle continues to work inside the VM.

## Overnight operation

Use `tmux` in the VM so terminal disconnects do not stop the run:

```bash
./scripts/lima-gardener.sh shell
# inside VM
tmux new -s gardener
cd /workspace/gardener
. "$HOME/.cargo/env"
cargo run -p gardener --bin gardener -- --config gardener.toml
```

Detach: `Ctrl-b d`

Reattach later:

```bash
./scripts/lima-gardener.sh shell
# inside VM
tmux attach -t gardener
```

## Stop or tear down

```bash
./scripts/lima-gardener.sh stop
./scripts/lima-gardener.sh delete
```

## Important caveat: host-runnable binaries

Build artifacts produced inside the Linux VM are Linux binaries and are not natively runnable on macOS. If a task requires running a binary on macOS, perform a host-native build/test after pulling the changes.
