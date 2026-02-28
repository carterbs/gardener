#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

CUSTOM_LINTERS=(
  "scripts/check-skills-sync.sh"
  "scripts/check-no-warnings.sh"
)

for linter in "${CUSTOM_LINTERS[@]}"; do
  echo "Running custom linter: $linter"
  "$linter"
done

echo "Running project validation command: cargo test -p gardener --all-targets"
cargo test -p gardener --all-targets

echo "Running project validation command: scripts/check-no-warnings.sh"
scripts/check-no-warnings.sh

# ---------- data-driven clippy lints from clippy-lints.toml ----------
# Each [[lints]] block defines a single flag.  The script groups them by scope
# and runs one cargo clippy invocation per scope.  Adding a new lint = appending
# a new [[lints]] block; git auto-merges independent appends without conflicts.

LINT_FILE="clippy-lints.toml"
if [[ ! -f "$LINT_FILE" ]]; then
  echo "ERROR: $LINT_FILE not found" >&2
  exit 1
fi

declare -a all_targets_flags=()
declare -a lib_bins_flags=()
current_scope=""

while IFS= read -r line; do
  # skip comments, blank lines, and section headers
  [[ "$line" =~ ^[[:space:]]*# ]] && continue
  [[ -z "$line" || "$line" == "[[lints]]" ]] && continue

  if [[ "$line" =~ ^scope[[:space:]]*=[[:space:]]*\"(.+)\"$ ]]; then
    current_scope="${BASH_REMATCH[1]}"
  elif [[ "$line" =~ ^level[[:space:]]*=[[:space:]]*\"(.+)\"$ ]]; then
    current_level="${BASH_REMATCH[1]}"
  elif [[ "$line" =~ ^name[[:space:]]*=[[:space:]]*\"(.+)\"$ ]]; then
    current_name="${BASH_REMATCH[1]}"
  fi

  # When we have all three fields, emit the flag and reset
  if [[ -n "${current_scope:-}" && -n "${current_level:-}" && -n "${current_name:-}" ]]; then
    case "$current_level" in
      warn) flag="-W" ;;
      deny) flag="-D" ;;
      *)    echo "ERROR: unknown lint level '$current_level'" >&2; exit 1 ;;
    esac
    case "$current_scope" in
      all-targets) all_targets_flags+=("$flag" "$current_name") ;;
      lib-bins)    lib_bins_flags+=("$flag" "$current_name") ;;
      *)           echo "ERROR: unknown lint scope '$current_scope'" >&2; exit 1 ;;
    esac
    current_scope="" current_level="" current_name=""
  fi
done < "$LINT_FILE"

if (( ${#all_targets_flags[@]} > 0 )); then
  echo "Running project validation command: cargo clippy -p gardener --all-targets -- ${all_targets_flags[*]}"
  cargo clippy -p gardener --all-targets -- "${all_targets_flags[@]}"
fi

if (( ${#lib_bins_flags[@]} > 0 )); then
  echo "Running project validation command: cargo clippy -p gardener --lib --bins -- ${lib_bins_flags[*]}"
  cargo clippy -p gardener --lib --bins -- "${lib_bins_flags[@]}"
fi
