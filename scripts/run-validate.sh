#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

CUSTOM_LINTERS=(
  "scripts/check-skills-sync.sh"
)

for linter in "${CUSTOM_LINTERS[@]}"; do
  echo "Running custom linter: $linter"
  "$linter"
done

echo "Running project validation command: cargo test -p gardener --all-targets"
cargo test -p gardener --all-targets

echo "Running project validation command: cargo clippy -p gardener --all-targets"
cargo clippy -p gardener --all-targets
