#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MIN_LINE_COVERAGE="${COVERAGE_MIN_LINE:-85}"
STRICT_UNIT_CORE_LINE_COVERAGE="${STRICT_UNIT_CORE_LINE_COVERAGE:-90}"
PROFILE_DIR="${COVERAGE_PROFILE_DIR:-target/llvm-cov-target/profraw}"
COVERAGE_IGNORE_MANIFEST="${COVERAGE_IGNORE_MANIFEST:-$SCRIPT_DIR/coverage-ignore-manifest.txt}"
COVERAGE_LCOV_FILE="${COVERAGE_LCOV_FILE:-target/llvm-cov-target/coverage/lcov.info}"
COVERAGE_HTML_DIR="${COVERAGE_HTML_DIR:-target/llvm-cov-target/coverage/html}"
TESTABILITY_BOUNDARY_MANIFEST="${TESTABILITY_BOUNDARY_MANIFEST:-$SCRIPT_DIR/../tools/gardener/testability-boundaries.toml}"
COVERAGE_DIFF_BASE_REF="${COVERAGE_DIFF_BASE_REF:-origin/main}"

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

coverage_report_args=( report -p gardener )
if [[ -n "$COVERAGE_IGNORE_REGEX" ]]; then
  coverage_report_args+=( --ignore-filename-regex "$COVERAGE_IGNORE_REGEX" )
fi

# Keep raw LLVM profiles out of the repo root when coverage-instrumented
# subprocesses are spawned during tests.
mkdir -p "$PROFILE_DIR"
export LLVM_PROFILE_FILE="${LLVM_PROFILE_FILE:-$(pwd)/$PROFILE_DIR/default_%p_%m.profraw}"

coverage_report_file="$(mktemp)"
changed_files_file="$(mktemp)"
trap 'rm -f "$coverage_report_file" "$changed_files_file"' EXIT

cargo_args=(llvm-cov "${coverage_llvm_cov_args[@]}" --json --summary-only)

report="$(cargo "${cargo_args[@]}")"
printf '%s\n' "$report" > "$coverage_report_file"

python3 - \
  "$coverage_report_file" \
  "$TESTABILITY_BOUNDARY_MANIFEST" \
  "$changed_files_file" \
  "$MIN_LINE_COVERAGE" \
  "$STRICT_UNIT_CORE_LINE_COVERAGE" \
  "$COVERAGE_DIFF_BASE_REF" <<'PY'
import json
import os
import re
import subprocess
import sys
from pathlib import Path

report_path = Path(sys.argv[1])
manifest_path = Path(sys.argv[2])
changed_files_path = Path(sys.argv[3])
min_line = float(sys.argv[4])
strict_unit_core = float(sys.argv[5])
diff_base_ref = sys.argv[6]
repo_root = Path.cwd()

raw = report_path.read_text(encoding="utf-8")
stripped = raw.lstrip()

def fail(message: str) -> None:
    print(message, file=sys.stderr)
    sys.exit(1)

def normalize_path(path: str) -> str:
    value = path.replace("\\", "/")
    root = repo_root.as_posix().rstrip("/")
    if value.startswith(root + "/"):
        value = value[len(root) + 1 :]
    src_root = "tools/gardener/src/"
    if value.startswith(src_root):
        return value[len(src_root) :]
    return value

def parse_total_from_text(text: str) -> float:
    for line in text.splitlines():
        if line.startswith("TOTAL"):
            fields = line.split()
            if fields:
                return float(fields[-1].rstrip("%"))
    raise ValueError("TOTAL line coverage not found")

if not stripped.startswith("{"):
    try:
        line_cov = parse_total_from_text(raw)
    except ValueError as exc:
        fail(f"coverage gate: could not parse TOTAL line coverage ({exc})")
    if line_cov < min_line:
        fail(
            f"coverage gate failed: line coverage {line_cov:.2f}% < {min_line:.1f}%\n"
            "Agent instruction: do not add files to scripts/coverage-ignore-manifest.txt, broaden ignore regexes, or delete code/tests to make coverage pass without explicit user consent first."
        )
    print(raw.rstrip())
    print(f"coverage gate passed: line coverage {line_cov:.2f}% >= {min_line:.1f}%")
    print("coverage gate note: changed-file manifest checks skipped because cargo llvm-cov did not emit JSON")
    sys.exit(0)

try:
    payload = json.loads(raw)
except json.JSONDecodeError as exc:
    fail(f"coverage gate: invalid JSON from cargo llvm-cov: {exc}")

data = payload.get("data") or []
if not data:
    fail("coverage gate: coverage JSON did not include data entries")

ignore_exact = set()
ignore_dirs = []
for line in manifest_path.read_text(encoding="utf-8").splitlines():
    line = line.strip()
    if not line or line.startswith("#"):
        continue
    if line.endswith("/"):
        ignore_dirs.append(line[:-1])
    else:
        ignore_exact.add(line)

def is_ignored(path: str) -> bool:
    if path in ignore_exact:
        return True
    return any(path == prefix or path.startswith(f"{prefix}/") for prefix in ignore_dirs)

files = {}
total_lines = 0
covered_lines = 0
for datum in data:
    for entry in datum.get("files", []):
        rel_path = normalize_path(entry.get("filename", ""))
        if is_ignored(rel_path):
            continue
        summary = entry.get("summary", {})
        files[rel_path] = summary
        lines = summary.get("lines", {})
        total_lines += int(lines.get("count", 0))
        covered_lines += int(lines.get("covered", 0))

if total_lines == 0:
    fail("coverage gate: no non-ignored source files were found in coverage JSON")

line_cov = (covered_lines / total_lines) * 100.0
print(f"TOTAL line coverage: {line_cov:.2f}%")
if line_cov < min_line:
    fail(
        f"coverage gate failed: line coverage {line_cov:.2f}% < {min_line:.1f}%\n"
        "Agent instruction: do not add files to scripts/coverage-ignore-manifest.txt, broaden ignore regexes, or delete code/tests to make coverage pass without explicit user consent first."
    )

if not manifest_path.is_file():
    fail(f"testability boundary manifest not found: {manifest_path}")

merge_base = subprocess.run(
    ["git", "merge-base", "HEAD", diff_base_ref],
    cwd=repo_root,
    text=True,
    capture_output=True,
)
if merge_base.returncode != 0:
    fail(
        "coverage gate: failed to resolve merge-base against "
        f"{diff_base_ref}\nstdout:\n{merge_base.stdout}\nstderr:\n{merge_base.stderr}"
    )
merge_base_sha = merge_base.stdout.strip()

changed = subprocess.run(
    ["git", "diff", "--name-only", "--diff-filter=AMR", "--find-renames", f"{merge_base_sha}...HEAD"],
    cwd=repo_root,
    text=True,
    capture_output=True,
)
if changed.returncode != 0:
    fail(
        "coverage gate: failed to enumerate changed files\n"
        f"stdout:\n{changed.stdout}\nstderr:\n{changed.stderr}"
    )
changed_files = [line.strip().replace("\\", "/") for line in changed.stdout.splitlines() if line.strip()]
changed_files_path.write_text("\n".join(changed_files), encoding="utf-8")

try:
    import tomllib
except ModuleNotFoundError as exc:
    fail(f"coverage gate: python tomllib is required: {exc}")

manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
entries = {}
for entry in manifest.get("file", []):
    entries[entry["path"]] = entry

changed_unit_core = []
changed_boundary = []
for path in changed_files:
    entry = entries.get(path)
    if not entry:
        continue
    if entry.get("role") == "unit-core":
        changed_unit_core.append((path, entry))
    elif entry.get("role") == "boundary-orchestration":
        changed_boundary.append((path, entry))

def line_summary(path: str):
    summary = files.get(path, {})
    lines = summary.get("lines", {})
    return float(lines.get("percent", 0.0)), int(lines.get("covered", 0)), int(lines.get("count", 0))

failures = []
if changed_unit_core:
    print("changed unit-core files:")
    for path, _entry in changed_unit_core:
        percent, covered, count = line_summary(path)
        print(f"  - {path}: {percent:.2f}% ({covered}/{count})")
        if percent + 1e-9 < strict_unit_core:
            failures.append(
                f"unit-core coverage gate failed for {path}: {percent:.2f}% < {strict_unit_core:.1f}% ({covered}/{count})"
            )

if changed_boundary:
    print("changed boundary-orchestration files:")
    for path, entry in changed_boundary:
        percent, covered, count = line_summary(path)
        owning_tests = entry.get("owning_tests") or []
        boundary_modes = entry.get("boundary_modes") or []
        print(
            f"  - {path}: {percent:.2f}% ({covered}/{count}), owners={','.join(owning_tests) or '<none>'}"
        )
        if not owning_tests:
            failures.append(f"boundary ownership gate failed for {path}: owning_tests is empty")
        if not boundary_modes:
            failures.append(f"boundary ownership gate failed for {path}: boundary_modes is empty")
        if covered <= 0:
            failures.append(
                f"boundary execution evidence gate failed for {path}: file has zero covered lines in coverage output"
            )

if failures:
    fail("\n".join(failures))

print(f"coverage gate passed: line coverage {line_cov:.2f}% >= {min_line:.1f}%")
if changed_unit_core or changed_boundary:
    print(f"coverage gate merge-base: {merge_base_sha}")
else:
    print("coverage gate note: no changed manifest-classified files relative to merge-base")
PY

mkdir -p "$(dirname "$COVERAGE_LCOV_FILE")"
cargo llvm-cov "${coverage_report_args[@]}" --lcov --output-path "$COVERAGE_LCOV_FILE"
echo "coverage lcov report: $COVERAGE_LCOV_FILE"

mkdir -p "$COVERAGE_HTML_DIR"
cargo llvm-cov "${coverage_report_args[@]}" --html --output-dir "$COVERAGE_HTML_DIR"
echo "coverage html report: $COVERAGE_HTML_DIR"
