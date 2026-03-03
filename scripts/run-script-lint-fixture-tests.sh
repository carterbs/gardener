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

run_expect_exit_capture() {
  local expected_exit=$1
  local output=$2
  shift 2

  set +e
  "$@" >"$output" 2>&1
  local status=$?
  set -e

  if [[ $status -ne $expected_exit ]]; then
    echo "command failed expectation" >&2
    echo "expected exit code: $expected_exit" >&2
    echo "actual exit code: $status" >&2
    echo "command: $*" >&2
    echo "output:" >&2
    cat "$output" >&2
    return 1
  fi
}

create_backlog_db() {
  local db_path=$1

  sqlite3 "$db_path" <<'SQL'
CREATE TABLE backlog_tasks (
    task_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    details TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    priority TEXT NOT NULL CHECK(priority IN ('P0', 'P1', 'P2')),
    status TEXT NOT NULL CHECK(status IN ('ready', 'leased', 'in_progress', 'merge_pending', 'complete', 'failed', 'unresolved')),
    last_updated INTEGER NOT NULL,
    lease_owner TEXT,
    lease_expires_at INTEGER,
    source TEXT NOT NULL,
    related_pr INTEGER,
    related_branch TEXT,
    rationale TEXT NOT NULL DEFAULT '',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
SQL
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

# recurring doc-gardening maintenance checks
doc_gardening_output="$(mktemp)"
repo_root="$(git -C "$SCRIPT_DIR/.." rev-parse --show-toplevel)"
quality_grades_path="$repo_root/docs/quality-grades.md"
quality_grades_stamp="${quality_grades_path}.stamp"
restore_quality_stamp=0
quality_stamp_backup="$(mktemp)"
if [ -f "$quality_grades_stamp" ]; then
  cp "$quality_grades_stamp" "$quality_stamp_backup"
  restore_quality_stamp=1
else
  rm -f "$quality_stamp_backup"
fi
echo "$(date -u +%s)" > "$quality_grades_stamp"

set +e
"$SCRIPT_DIR/doc-gardening.sh" > "$doc_gardening_output" 2>&1
doc_gardening_status=$?
set -e

if [[ $doc_gardening_status -ne 0 ]]; then
  echo "doc-gardening checks failed" >&2
  sed 's/^/  /' "$doc_gardening_output" >&2
  rm -f "$doc_gardening_output"
  exit 1
fi
if [[ $restore_quality_stamp -eq 1 ]]; then
  mv "$quality_stamp_backup" "$quality_grades_stamp"
else
  rm -f "$quality_grades_stamp"
  rm -f "$quality_stamp_backup"
fi
assert_file_contains "$doc_gardening_output" "Doc-gardening summary:"
rm -f "$doc_gardening_output"

tmp_backlog_dir="$(mktemp -d)"
backlog_db="$tmp_backlog_dir/backlog.sqlite"
missing_backlog_db="$tmp_backlog_dir/missing.sqlite"
create_backlog_db "$backlog_db"

backlog_output="$(mktemp)"
run_expect_exit_capture 1 "$backlog_output" \
  "$SCRIPT_DIR/backlog-db.sh" list --db "$missing_backlog_db"
assert_file_contains "$backlog_output" "database file not found"

run_expect_exit_capture 1 "$backlog_output" \
  "$SCRIPT_DIR/backlog-db.sh" add --title "Manual task" --details "details" --db "$missing_backlog_db"
assert_file_contains "$backlog_output" "database file not found"

run_expect_exit_capture 1 "$backlog_output" \
  "$SCRIPT_DIR/backlog-db.sh" add --title "Manual task" --priority "P3" --details "details" --db "$backlog_db"
assert_file_contains "$backlog_output" "invalid --priority"

run_expect_exit_capture 1 "$backlog_output" \
  "$SCRIPT_DIR/backlog-db.sh" add --title "Manual task" --details "details" --status "stale" --db "$backlog_db"
assert_file_contains "$backlog_output" "invalid --status"

run_expect_exit_capture 1 "$backlog_output" \
  "$SCRIPT_DIR/backlog-db.sh" add --title "Manual task" --details "details" --kind "Feature" --db "$backlog_db"
assert_file_contains "$backlog_output" "invalid --kind"

create_fake_toolchain() {
  local target_dir=$1
  local repo_root=$2
  local include_gh=$3

  rm -rf "$target_dir"
  mkdir -p "$target_dir/bin"

  cat > "$target_dir/bin/git" <<'SH'
#!/usr/bin/env bash
if [[ "$1" == "rev-parse" && "$2" == "--show-toplevel" ]]; then
  echo "$GARDENER_FAKE_REPO_ROOT"
  exit 0
fi
if [[ "$1" == "config" ]]; then
  exit 0
fi
exit 0
SH

  cat > "$target_dir/bin/file" <<'SH'
#!/usr/bin/env bash
exit 0
SH

  cat > "$target_dir/bin/cargo" <<'SH'
#!/usr/bin/env bash
if [[ "$1" == "fmt" || "$1" == "clippy" || "$1" == "llvm-cov" ]]; then
  exit 0
fi
if [[ "$1" == "install" ]]; then
  exit 0
fi
exit 0
SH

  cat > "$target_dir/bin/rustfmt" <<'SH'
#!/usr/bin/env bash
exit 0
SH

  cat > "$target_dir/bin/cargo-llvm-cov" <<'SH'
#!/usr/bin/env bash
exit 0
SH

  if [[ "$include_gh" == "1" ]]; then
    cat > "$target_dir/bin/gh" <<'SH'
#!/usr/bin/env bash
exit 0
SH
  else
    cat > "$target_dir/bin/gh" <<'SH'
#!/usr/bin/env bash
exit 127
SH
  fi

  chmod +x "$target_dir/bin/"*
}

run_preflight_tool_dir="$(mktemp -d)"
run_preflight_success_output="$(mktemp)"
trap 'rm -rf "$run_preflight_tool_dir" "$run_preflight_success_output" "$run_preflight_missing_output"' EXIT

create_fake_toolchain "$run_preflight_tool_dir" "$SCRIPT_DIR/.." 1
run_expect_exit_capture 0 "$run_preflight_success_output" \
  env \
    GARDENER_FAKE_REPO_ROOT="$SCRIPT_DIR/.." \
    PATH="$run_preflight_tool_dir/bin:/bin" \
    bash "$SCRIPT_DIR/run-validate.sh" --preflight
assert_file_contains "$run_preflight_success_output" "Pre-flight checks passed"

create_fake_toolchain "$run_preflight_tool_dir" "$SCRIPT_DIR/.." 0
run_preflight_missing_output="$(mktemp)"
run_expect_exit_capture 1 "$run_preflight_missing_output" \
  env \
    GARDENER_FAKE_REPO_ROOT="$SCRIPT_DIR/.." \
    PATH="$run_preflight_tool_dir/bin:/bin" \
    bash "$SCRIPT_DIR/run-validate.sh" --preflight
assert_file_contains "$run_preflight_missing_output" "Pre-flight failed"
assert_file_contains "$run_preflight_missing_output" "gh"
assert_file_contains "$run_preflight_missing_output" "Install GitHub CLI"
assert_file_contains "$run_preflight_missing_output" "Example: ./scripts/setup-git-hooks.sh --preflight"

run_preflight_missing_output="$(mktemp)"
run_expect_exit_capture 1 "$run_preflight_missing_output" \
  env \
    GARDENER_FAKE_REPO_ROOT="$SCRIPT_DIR/.." \
    PATH="$run_preflight_tool_dir/bin:/bin" \
    bash "$SCRIPT_DIR/setup-git-hooks.sh" --preflight
assert_file_contains "$run_preflight_missing_output" "Pre-flight failed"
assert_file_contains "$run_preflight_missing_output" "gh"

run_expect_exit_capture 1 "$backlog_output" \
  "$SCRIPT_DIR/backlog-db.sh" add --details "details" --db "$backlog_db"
assert_file_contains "$backlog_output" "--title and --details are required for add"

run_expect_exit_capture 1 "$backlog_output" \
  "$SCRIPT_DIR/backlog-db.sh" add --title "Manual task" --db "$backlog_db"
assert_file_contains "$backlog_output" "--title and --details are required for add"

rm -f "$backlog_output"
rm -rf "$tmp_backlog_dir"

echo "Script lint fixture tests completed successfully."
