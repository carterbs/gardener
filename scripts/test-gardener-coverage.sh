#!/usr/bin/env bash
set -euo pipefail

MIN_LINE_COVERAGE="${COVERAGE_MIN_LINE:-80}"

report="$(cargo llvm-cov -p gardener --all-targets --summary-only)"
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
