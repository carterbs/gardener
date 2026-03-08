#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  repo_root="${GARDENER_REPO_ROOT:-"$(git rev-parse --show-toplevel)"}"
  cd "$repo_root"
fi

declare -a failed_checks=()
checks_total=0
checks_passed=0
checks_failed=0

extract_toml_value() {
  local section=$1
  local key=$2
  local file=$3

  awk -v section="$section" -v key="$key" '
    function trim(s) {
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", s);
      return s;
    }

    $0 ~ "^\\[" section "\\][[:space:]]*$" {
      in_section = 1;
      next;
    }

    in_section && $0 ~ "^\\[[^]]+\\][[:space:]]*$" {
      exit;
    }

    in_section && $0 ~ ("^[[:space:]]*" key "[[:space:]]*=") {
      value = $0;
      sub(/^[^=]*=[[:space:]]*/, "", value);
      sub(/#.*/, "", value);
      value = trim(value);
      if (value ~ /^".*"$/) {
        sub(/^"/, "", value);
        sub(/"$/, "", value);
      }
      if (value ~ /^'.*'$/) {
        sub(/^'\''/, "", value);
        sub(/'\''$/, "", value);
      }
      print value;
      exit;
    }
  ' "$file"
}

run_check() {
  local name=$1
  shift

  checks_total=$((checks_total + 1))
  echo "Running doc-gardening check: $name"

  local output status
  set +e
  output="$("$@" 2>&1)"
  status=$?
  set -e

  if [[ $status -eq 0 ]]; then
    checks_passed=$((checks_passed + 1))
    echo "  ok: $name"
  else
    checks_failed=$((checks_failed + 1))
    failed_checks+=("$name")
    echo "  fail: $name" >&2
    echo "$output" >&2
  fi
}

check_quality_grade_freshness() {
  local name="quality-grade freshness checks"
  local quality_path quality_stamp stale_after_days
  local stamp_value now_seconds ttl_seconds age

  checks_total=$((checks_total + 1))
  echo "Running doc-gardening check: $name"

  quality_path="$(extract_toml_value "quality_report" "path" "$repo_root/gardener.toml")"
  if [[ -z "$quality_path" ]]; then
    quality_path="docs/quality-grades.md"
  fi
  if [[ "${quality_path}" != /* ]]; then
    quality_path="$repo_root/$quality_path"
  fi
  quality_stamp="${quality_path}.stamp"

  if [[ ! -f "$quality_path" ]]; then
    checks_failed=$((checks_failed + 1))
    failed_checks+=("$name")
    echo "  fail: $name" >&2
    echo "quality report missing: $quality_path" >&2
    return 0
  fi

  if [[ ! -f "$quality_stamp" ]]; then
    checks_failed=$((checks_failed + 1))
    failed_checks+=("$name")
    echo "  fail: $name" >&2
    echo "quality report stamp missing: $quality_stamp" >&2
    return 0
  fi

  stamp_value="$(tr -dc '0-9' < "$quality_stamp")"
  if [[ -z "$stamp_value" ]]; then
    checks_failed=$((checks_failed + 1))
    failed_checks+=("$name")
    echo "  fail: $name" >&2
    echo "quality report stamp is not a unix timestamp: $quality_stamp" >&2
    return 0
  fi

  stale_after_days="$(extract_toml_value "quality_report" "stale_after_days" "$repo_root/gardener.toml")"
  stale_after_days="${stale_after_days:-7}"
  case "$stale_after_days" in
    ''|*[!0-9]*)
      stale_after_days=7
      ;;
  esac

  now_seconds="$(date -u +%s)"
  ttl_seconds=$((stale_after_days * 24 * 60 * 60))
  age=$((now_seconds - stamp_value))
  if (( age > ttl_seconds )); then
    checks_failed=$((checks_failed + 1))
    failed_checks+=("$name")
    echo "  fail: $name" >&2
    echo "quality report stamp is older than configured TTL (${stale_after_days} days): $quality_stamp" >&2
    return 0
  fi

  checks_passed=$((checks_passed + 1))
  echo "  ok: $name"
}

run_check "doc/reference contract suite" \
  cargo test -p gardener --test docs_integration --test cli_integration

check_quality_grade_freshness

echo
echo "Doc-gardening summary: ${checks_passed}/${checks_total} checks passed."
if (( checks_failed > 0 )); then
  echo "Failed checks:"
  for check in "${failed_checks[@]}"; do
    echo "  - $check"
  done
  exit 1
fi
