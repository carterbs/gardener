#!/usr/bin/env bash
set -euo pipefail

MIN_LINE_COVERAGE="${COVERAGE_MIN_LINE:-90}"
COVERAGE_IGNORE_REGEX="${COVERAGE_IGNORE_REGEX:-"/tools/gardener/src/(worker\.rs|startup\.rs|tui\.rs|worker_pool\.rs|runtime/mod\.rs|backlog_store\.rs|git\.rs|worktree\.rs|lib\.rs|replay/replayer\.rs|seeding\.rs|triage\.rs|pr_audit\.rs)"}"

if [[ -n "$COVERAGE_IGNORE_REGEX" ]]; then
  report="$(cargo llvm-cov -p gardener --all-targets --summary-only --ignore-filename-regex "$COVERAGE_IGNORE_REGEX")"
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
