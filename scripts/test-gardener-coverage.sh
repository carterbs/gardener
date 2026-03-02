#!/usr/bin/env bash
set -euo pipefail

build_ignore_regex_from_manifest() {
  local manifest_path=$1
  local patterns=()

  if [[ ! -f "$manifest_path" ]]; then
    echo "coverage gate: missing coverage ignore manifest: ${manifest_path}" >&2
    return 1
  fi

  while IFS= read -r raw_line || [[ -n "$raw_line" ]]; do
    local line
    line="${raw_line%%#*}"
    line="${line#/}"
    line="$(printf '%s' "$line" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
    if [[ -z "$line" ]]; then
      continue
    fi
    patterns+=("$line")
  done < "$manifest_path"

  if [[ "${#patterns[@]}" -eq 0 ]]; then
    echo ""
    return 0
  fi

  printf "/("
  printf "%s" "${patterns[0]}"
  for pattern in "${patterns[@]:1}"; do
    printf "|%s" "$pattern"
  done
  printf ")"
}

MIN_LINE_COVERAGE="${COVERAGE_MIN_LINE:-90}"
if [[ -n "${COVERAGE_IGNORE_REGEX:-}" ]]; then
  COVERAGE_IGNORE_REGEX_EFFECTIVE="$COVERAGE_IGNORE_REGEX"
else
  COVERAGE_IGNORE_MANIFEST="${COVERAGE_IGNORE_MANIFEST:-scripts/coverage-ignore-manifest.txt}"
  COVERAGE_IGNORE_REGEX_EFFECTIVE="$(build_ignore_regex_from_manifest "$COVERAGE_IGNORE_MANIFEST")"
fi

if [[ "${COVERAGE_DRY_RUN:-}" == "1" ]]; then
  if [[ -n "$COVERAGE_IGNORE_REGEX_EFFECTIVE" ]]; then
    echo "coverage gate dry-run: cargo llvm-cov -p gardener --all-targets --summary-only --ignore-filename-regex $COVERAGE_IGNORE_REGEX_EFFECTIVE"
  else
    echo "coverage gate dry-run: cargo llvm-cov -p gardener --all-targets --summary-only"
  fi
  exit 0
fi

if [[ -n "$COVERAGE_IGNORE_REGEX_EFFECTIVE" ]]; then
  report="$(cargo llvm-cov -p gardener --all-targets --summary-only --ignore-filename-regex "$COVERAGE_IGNORE_REGEX_EFFECTIVE")"
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
