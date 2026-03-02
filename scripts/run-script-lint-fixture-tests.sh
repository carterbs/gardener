#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

run_expect_exit() {
  local expected_exit=$1
  shift

  local output
  set +e
  output="$($* 2>&1)"
  local status=$?
  set -e

  if [[ $status -ne $expected_exit ]]; then
    printf 'command failed expectation\n' >&2
    printf 'expected exit code: %s\n' "$expected_exit" >&2
    printf 'actual exit code: %s\n' "$status" >&2
    printf 'command: %s\n' "$*" >&2
    printf '%s\n' "$output" >&2
    return 1
  fi
}

# check-skills-sync fixtures
run_expect_exit 0 env \
  GARDENER_REPO_ROOT="$SCRIPT_DIR/fixtures/check-skills-sync/passing" \
  "$SCRIPT_DIR/check-skills-sync.sh"

run_expect_exit 1 env \
  GARDENER_REPO_ROOT="$SCRIPT_DIR/fixtures/check-skills-sync/missing-in-codex" \
  "$SCRIPT_DIR/check-skills-sync.sh"

run_expect_exit 1 env \
  GARDENER_REPO_ROOT="$SCRIPT_DIR/fixtures/check-skills-sync/different-content" \
  "$SCRIPT_DIR/check-skills-sync.sh"

# check-migrations-wired fixtures
run_expect_exit 0 env \
  GARDENER_REPO_ROOT="$SCRIPT_DIR/fixtures/check-migrations-wired/passing" \
  GARDENER_MIGRATIONS_DIR="$SCRIPT_DIR/fixtures/check-migrations-wired/passing/migrations" \
  GARDENER_MIGRATIONS_STORE_FILE="$SCRIPT_DIR/fixtures/check-migrations-wired/passing/src/backlog_store.rs" \
  "$SCRIPT_DIR/check-migrations-wired.sh"

run_expect_exit 1 env \
  GARDENER_REPO_ROOT="$SCRIPT_DIR/fixtures/check-migrations-wired/missing-migration" \
  GARDENER_MIGRATIONS_DIR="$SCRIPT_DIR/fixtures/check-migrations-wired/missing-migration/migrations" \
  GARDENER_MIGRATIONS_STORE_FILE="$SCRIPT_DIR/fixtures/check-migrations-wired/missing-migration/src/backlog_store.rs" \
  "$SCRIPT_DIR/check-migrations-wired.sh"

echo "Script lint fixture tests completed successfully."
