#!/usr/bin/env bash
set -euo pipefail

MIN_LINE_COVERAGE="${COVERAGE_MIN_LINE:-90}"
COVERAGE_IGNORE_REGEX="${COVERAGE_IGNORE_REGEX:-"/tools/gardener/src/(worker\.rs|worker/.*|startup\.rs|tui\.rs|worker_pool\.rs|runtime/mod\.rs|backlog_store\.rs|git\.rs|worktree\.rs|lib\.rs|replay/replayer\.rs|seeding\.rs|triage\.rs|pr_audit\.rs|agent_turn\.rs|do_phase\.rs|git_phase\.rs|merge_loop\.rs|phase_cli\.rs|plan_phase\.rs|review_phase\.rs|understand_phase\.rs|bin/do_task\.rs|bin/git_push\.rs|bin/plan\.rs|bin/review_pr\.rs|bin/understand\.rs|bin/friction_analysis\.rs)"}"
COVERAGE_IGNORE_MANIFEST="${COVERAGE_IGNORE_MANIFEST:-}"
PROFILE_DIR="${COVERAGE_PROFILE_DIR:-target/llvm-cov-target/profraw}"

# Keep raw LLVM profiles out of the repo root when coverage-instrumented
# subprocesses are spawned during tests.
mkdir -p "$PROFILE_DIR"
export LLVM_PROFILE_FILE="${LLVM_PROFILE_FILE:-$(pwd)/$PROFILE_DIR/default_%p_%m.profraw}"

ignore_args=()
if [[ -n "$COVERAGE_IGNORE_REGEX" ]]; then
  ignore_args+=(--ignore-filename-regex "$COVERAGE_IGNORE_REGEX")
fi

if [[ -n "$COVERAGE_IGNORE_MANIFEST" ]]; then
  if [[ ! -f "$COVERAGE_IGNORE_MANIFEST" ]]; then
    echo "coverage gate: cannot read COVERAGE_IGNORE_MANIFEST file: ${COVERAGE_IGNORE_MANIFEST}" >&2
    exit 1
  fi

  while IFS= read -r pattern; do
    pattern="${pattern#"${pattern%%[![:space:]]*}"}"
    pattern="${pattern%"${pattern##*[![:space:]]}"}"
    [[ -z "$pattern" || "$pattern" == \#* ]] && continue
    ignore_args+=(--ignore-filename-regex "$pattern")
  done < <(awk '{
    gsub(/^[[:space:]]+/, "", $0)
    gsub(/[[:space:]]+$/, "", $0)
    if ($0 != "" && $0 !~ /^#/) print
  }' "$COVERAGE_IGNORE_MANIFEST")
fi

if [[ ${#ignore_args[@]} -gt 0 ]]; then
  report="$(cargo llvm-cov -p gardener --all-targets --summary-only "${ignore_args[@]}")"
else
  report="$(cargo llvm-cov -p gardener --all-targets --summary-only)"
fi
printf '%s\n' "$report"

line_cov="$(printf '%s\n' "$report" | awk '/^TOTAL/{print $10}' | tr -d '%')"
if [[ -z "$line_cov" ]]; then
  echo "coverage gate: could not parse TOTAL line coverage" >&2
  exit 1
fi

awk -v got="$line_cov" -v min="$MIN_LINE_COVERAGE" 'BEGIN { if (got + 0 < min + 0) exit 1 }' || {
  echo "coverage gate failed: line coverage ${line_cov}% < ${MIN_LINE_COVERAGE}%" >&2
  exit 1
}

echo "coverage gate passed: line coverage ${line_cov}% >= ${MIN_LINE_COVERAGE}%"
