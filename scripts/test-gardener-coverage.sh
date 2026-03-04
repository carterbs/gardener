#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MIN_LINE_COVERAGE="${COVERAGE_MIN_LINE:-85}"
PROFILE_DIR="${COVERAGE_PROFILE_DIR:-target/llvm-cov-target/profraw}"
COVERAGE_IGNORE_MANIFEST="${COVERAGE_IGNORE_MANIFEST:-$SCRIPT_DIR/coverage-ignore-manifest.txt}"
COVERAGE_LCOV_FILE="${COVERAGE_LCOV_FILE:-target/llvm-cov-target/coverage/lcov.info}"
COVERAGE_HTML_DIR="${COVERAGE_HTML_DIR:-target/llvm-cov-target/coverage/html}"

escape_for_regex() {
  local value="$1"
  printf '%s' "$value" | sed -e 's#[.\\/+*?^$(){}|]#\\&#g'
}

build_coverage_ignore_regex() {
  local manifest_path="$1"
  local raw_patterns=()
  local final_patterns=()
  local pattern

  if [[ ! -f "$manifest_path" ]]; then
    echo "coverage ignore manifest not found: $manifest_path" >&2
    exit 1
  fi

  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    [[ -z "$line" || "${line:0:1}" == "#" ]] && continue

    if [[ "$line" == *" "* || "$line" == *$'\t'* ]]; then
      echo "coverage ignore manifest entry has whitespace: $line" >&2
      exit 1
    fi
    if [[ "$line" == *\** || "$line" == *\?* || "$line" == *"["* || "$line" == *"]"* ]]; then
      echo "coverage ignore manifest entries must be plain paths (no shell glob characters): $line" >&2
      exit 1
    fi
    if [[ "$line" == \/* || "$line" == *".."* ]]; then
      echo "coverage ignore manifest entries must be relative paths under tools/gardener/src/: $line" >&2
      exit 1
    fi

    raw_patterns+=("$line")
  done < "$manifest_path"

  if ((${#raw_patterns[@]} == 0)); then
    echo ""
    return
  fi

  local temp_patterns_file
  temp_patterns_file="$(mktemp)"
  for pattern in "${raw_patterns[@]}"; do
    local is_dir=0
    local normalized="${pattern}"

    if [[ "$normalized" == */ ]]; then
      normalized="${normalized%/}"
      is_dir="1"
    fi
    normalized="$(escape_for_regex "$normalized")"
    if [[ "$is_dir" -eq 1 ]]; then
      normalized="${normalized}/.*"
    fi
    printf '%s\n' "$normalized" >> "$temp_patterns_file"
  done

  while IFS= read -r pattern; do
    final_patterns+=("$pattern")
  done < <(sort -u "$temp_patterns_file")
  rm -f "$temp_patterns_file"

  local body
  body="$(printf '%s|' "${final_patterns[@]}")"
  body="${body%|}"
  echo "/tools/gardener/src/($body)"
}

COVERAGE_IGNORE_REGEX="${COVERAGE_IGNORE_REGEX:-$(build_coverage_ignore_regex "$COVERAGE_IGNORE_MANIFEST")}"

coverage_llvm_cov_args=( -p gardener --all-targets )
if [[ -n "$COVERAGE_IGNORE_REGEX" ]]; then
  coverage_llvm_cov_args+=( --ignore-filename-regex "$COVERAGE_IGNORE_REGEX" )
fi

# Keep raw LLVM profiles out of the repo root when coverage-instrumented
# subprocesses are spawned during tests.
mkdir -p "$PROFILE_DIR"
export LLVM_PROFILE_FILE="${LLVM_PROFILE_FILE:-$(pwd)/$PROFILE_DIR/default_%p_%m.profraw}"

report="$(cargo llvm-cov "${coverage_llvm_cov_args[@]}" --summary-only)"
printf '%s\n' "$report"

line_cov="$(printf '%s\n' "$report" | awk '/^TOTAL/{print $10}' | tr -d '%')"
if [[ -z "$line_cov" ]]; then
  echo "coverage gate: could not parse TOTAL line coverage" >&2
  exit 1
fi

mkdir -p "$(dirname "$COVERAGE_LCOV_FILE")"
cargo llvm-cov "${coverage_llvm_cov_args[@]}" --lcov --output-path "$COVERAGE_LCOV_FILE"
echo "coverage lcov report: $COVERAGE_LCOV_FILE"

mkdir -p "$COVERAGE_HTML_DIR"
cargo llvm-cov "${coverage_llvm_cov_args[@]}" --html --output-dir "$COVERAGE_HTML_DIR"
echo "coverage html report: $COVERAGE_HTML_DIR"

awk -v got="$line_cov" -v min="$MIN_LINE_COVERAGE" 'BEGIN { if (got + 0 < min + 0) exit 1 }' || {
  echo "coverage gate failed: line coverage ${line_cov}% < ${MIN_LINE_COVERAGE}%" >&2
  echo "Agent instruction: do not add files to scripts/coverage-ignore-manifest.txt, broaden ignore regexes, or delete code/tests to make coverage pass without explicit user consent first." >&2
  exit 1
}

echo "coverage gate passed: line coverage ${line_cov}% >= ${MIN_LINE_COVERAGE}%"
