#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  repo_root="${GARDENER_REPO_ROOT:-"$(git rev-parse --show-toplevel)"}"
  cd "$repo_root"
fi

exec "$repo_root/scripts/doc-gardening.sh"
