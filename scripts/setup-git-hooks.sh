#!/usr/bin/env bash
set -euo pipefail

show_usage() {
  cat <<'EOF'
Usage: ./scripts/setup-git-hooks.sh [--preflight]

Configure local pre-commit hook plumbing.
Pass --preflight to validate required tooling before setting up hooks.
EOF
}

run_preflight=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --preflight)
      run_preflight=1
      shift
      ;;
    -h|--help)
      show_usage
      exit 0
      ;;
    *)
      echo "Unsupported argument: $1" >&2
      show_usage >&2
      exit 64
      ;;
  esac
done

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

if [[ "$run_preflight" -eq 1 ]]; then
  ./scripts/run-validate.sh --preflight
  exit $?
fi

git config core.hooksPath .githooks
echo "Configured Git hooks path to .githooks"
