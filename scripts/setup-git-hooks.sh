#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

git config core.hooksPath .githooks
echo "Configured Git hooks path to .githooks"
