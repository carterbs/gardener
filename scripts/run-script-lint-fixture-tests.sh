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

assert_file_contains() {
  local file=$1
  local expected=$2

  if ! grep -Fq -- "$expected" "$file"; then
    echo "fixture assertion failed" >&2
    echo "expected to find: $expected" >&2
    echo "in: $file" >&2
    echo "actual contents:" >&2
    sed -n '1,120p' "$file" >&2
    return 1
  fi
}

assert_file_not_contains() {
  local file=$1
  local unexpected=$2

  if grep -Fq -- "$unexpected" "$file"; then
    echo "fixture assertion failed" >&2
    echo "unexpectedly found: $unexpected" >&2
    echo "in: $file" >&2
    echo "actual contents:" >&2
    sed -n '1,120p' "$file" >&2
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

# check-binary-blobs fixtures
run_expect_exit 0 "$SCRIPT_DIR/check-binary-blobs.sh" \
  "$SCRIPT_DIR/fixtures/check-binary-blobs/passing/runtime-note.txt"

binary_blob_dir="$(mktemp -d)"
blocked_profraw="$binary_blob_dir/default_1234567890_0_00000.profraw"
blocked_startup_diag_dir="$binary_blob_dir/startup-diagnostics"
blocked_startup_diag="$blocked_startup_diag_dir/runtime-startup.md"

printf "runtime profile data\n" > "$blocked_profraw"
mkdir -p "$blocked_startup_diag_dir"
printf "# Runtime startup diagnostics\n" > "$blocked_startup_diag"

binary_blob_output="$(mktemp)"
set +e
"$SCRIPT_DIR/check-binary-blobs.sh" \
  "$blocked_profraw" \
  "$blocked_startup_diag" \
  >"$binary_blob_output" 2>&1
status=$?
set -e
if [[ $status -ne 1 ]]; then
  echo "expected check-binary-blobs to reject generated runtime artifacts" >&2
  sed -n '1,120p' "$binary_blob_output" >&2
  rm -f "$binary_blob_output"
  rm -rf "$binary_blob_dir"
  exit 1
fi
assert_file_contains "$binary_blob_output" "default_1234567890_0_00000.profraw"
assert_file_contains "$binary_blob_output" "runtime-startup.md"
rm -f "$binary_blob_output"
rm -rf "$binary_blob_dir"

# startup-diagnostics fixtures
start_diag_output_1="$(mktemp)"
run_expect_exit 0 "$SCRIPT_DIR/startup-diagnostics.sh" \
  --run-id "run-123" \
  --log-path "$SCRIPT_DIR/fixtures/startup-diagnostics/passing/logs.ndjson" \
  --output "$start_diag_output_1" \
  --stage "startup-audits" \
  --error "initialization_failed"
assert_file_contains "$start_diag_output_1" "# Startup diagnostics"
assert_file_contains "$start_diag_output_1" "- stage: startup-audits"
assert_file_contains "$start_diag_output_1" "- run_id: run-123"
assert_file_contains "$start_diag_output_1" "startup.phase: {\"phase\":\"seeded\",\"step\":1}"
assert_file_contains "$start_diag_output_1" "boot.stage.init: {\"status\":\"ok\",\"trace\":\"abc\"}"
assert_file_contains "$start_diag_output_1" "run.failed: {\"error\":\"boom\"}"
assert_file_contains "$start_diag_output_1" "run.completed: {\"exit_code\":0}"
assert_file_not_contains "$start_diag_output_1" "other-run"
rm -f "$start_diag_output_1"

start_diag_output_2="$(mktemp)"
run_expect_exit 0 "$SCRIPT_DIR/startup-diagnostics.sh" \
  --run-id "run-456" \
  --log-path "$SCRIPT_DIR/fixtures/startup-diagnostics/malformed-json/logs.ndjson" \
  --output "$start_diag_output_2" \
  --stage "startup-audits" \
  --error "malformed_json"
assert_file_contains "$start_diag_output_2" "Could not parse log file as JSONL."
rm -f "$start_diag_output_2"

start_diag_output_3="$(mktemp)"
run_expect_exit 0 "$SCRIPT_DIR/startup-diagnostics.sh" \
  --run-id "run-789" \
  --log-path "$SCRIPT_DIR/fixtures/startup-diagnostics/missing/does-not-exist.jsonl" \
  --output "$start_diag_output_3" \
  --stage "startup-audits" \
  --error "missing_logs"
assert_file_contains "$start_diag_output_3" "No log file available for this run."
rm -f "$start_diag_output_3"

echo "Script lint fixture tests completed successfully."
