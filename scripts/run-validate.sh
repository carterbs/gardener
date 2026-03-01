#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

CUSTOM_LINTERS=(
  "scripts/check-skills-sync.sh"
  "scripts/check-no-warnings.sh"
  "scripts/check-migrations-wired.sh"
)

for linter in "${CUSTOM_LINTERS[@]}"; do
  echo "Running custom linter: $linter"
  "$linter"
done

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "Installing coverage tool: cargo-llvm-cov --locked"
  cargo install cargo-llvm-cov --locked
fi

echo "Running project validation command: ./scripts/test-gardener-coverage.sh"
./scripts/test-gardener-coverage.sh
