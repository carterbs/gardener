#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

echo "Running no-warning clippy check: cargo clippy -p gardener --all-targets -- -D warnings -A clippy::expect_used -W clippy::redundant_static_lifetimes"
cargo clippy -p gardener --all-targets -- -D warnings -A clippy::expect_used -W clippy::redundant_static_lifetimes
