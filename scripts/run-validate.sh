#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

show_usage() {
  cat <<'EOF'
Usage: ./scripts/run-validate.sh [--preflight]

Run validation stages. Pass --preflight to only validate required tooling before
running the full pipeline.
EOF
}

run_preflight() {
  local missing=0
  local missing_tools=()
  local recommendations=()

  if ! command -v git >/dev/null 2>&1; then
    missing_tools+=("git")
    recommendations+=("Install Git: https://git-scm.com/downloads")
  fi

  if ! command -v cargo >/dev/null 2>&1; then
    missing_tools+=("cargo")
    recommendations+=("Install Rust and Cargo: https://rustup.rs")
  fi

  if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
    missing_tools+=("cargo-llvm-cov")
    recommendations+=("Install coverage helper: cargo install cargo-llvm-cov --locked")
  fi

  if ! gh --version >/dev/null 2>&1; then
    missing_tools+=("gh")
    recommendations+=("Install GitHub CLI: https://cli.github.com/manual/installation")
  fi

  if ! command -v file >/dev/null 2>&1; then
    missing_tools+=("file")
    recommendations+=("Install file(1): package manager (e.g. apt install file / brew install file)")
  fi

  if ! command -v rustfmt >/dev/null 2>&1 && ! cargo fmt --version >/dev/null 2>&1; then
    missing_tools+=("rustfmt")
    recommendations+=("Install rustfmt: rustup component add rustfmt")
  fi

  if ! cargo clippy --version >/dev/null 2>&1; then
    missing_tools+=("clippy")
    recommendations+=("Install clippy: rustup component add clippy")
  fi

  if [[ ${#missing_tools[@]} -gt 0 ]]; then
    echo "Pre-flight failed: missing required tooling for pre-commit validation."
    echo "Install these tools and rerun with --preflight before committing:"
    for i in "${!missing_tools[@]}"; do
      printf '  - %s\n' "${missing_tools[$i]}"
      printf '    %s\n' "${recommendations[$i]}"
    done
    echo "Example: ./scripts/setup-git-hooks.sh --preflight"
    return 1
  fi

  echo "Pre-flight checks passed: validation tooling is available."
  return 0
}

if [[ "${1-}" == "--help" || "${1-}" == "-h" ]]; then
  show_usage
  exit 0
fi

if [[ "${1-}" == "--preflight" ]]; then
  run_preflight
  exit $?
fi

if [[ "${1-}" != "" ]]; then
  echo "Unsupported argument: ${1-}" >&2
  show_usage >&2
  exit 64
fi

_validate_branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "detached")"
VALIDATE_LOG="/tmp/gardener-validate-${_validate_branch//\//-}.log"
: > "$VALIDATE_LOG"

run_step() {
  local name="$1"
  shift
  local start_sec
  start_sec="$(date +%s)"
  if "$@" >> "$VALIDATE_LOG" 2>&1; then
    local elapsed=$(( $(date +%s) - start_sec ))
    printf '[%s]: OK (%ds)\n' "$name" "$elapsed"
  else
    local rc=$?
    local elapsed=$(( $(date +%s) - start_sec ))
    printf '[%s]: FAILED (%ds) — full log at %s\n' "$name" "$elapsed" "$VALIDATE_LOG" >&2
    exit $rc
  fi
}

CUSTOM_LINTERS=(
  "scripts/doc-gardening.sh"
  "scripts/check-skills-sync.sh"
  "scripts/check-migrations-wired.sh"
  "scripts/check-binary-blobs.sh"
)

for linter in "${CUSTOM_LINTERS[@]}"; do
  run_step "${linter#scripts/}" "$linter"
done

run_step "preflight" run_preflight

run_step "test-gardener-coverage" ./scripts/test-gardener-coverage.sh

CUSTOM_LINTERS=(
  "scripts/check-no-warnings.sh"
  "scripts/run-script-lint-fixture-tests.sh"
)

for linter in "${CUSTOM_LINTERS[@]}"; do
  run_step "${linter#scripts/}" "$linter"
done
