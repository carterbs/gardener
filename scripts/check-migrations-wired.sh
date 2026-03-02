#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

migrations_dir="tools/gardener/migrations"
store_file="tools/gardener/src/backlog_store.rs"
exit_code=0

for sql_file in "$migrations_dir"/*.sql; do
  basename=$(basename "$sql_file")
  if ! grep -q "include_str!(\"../migrations/$basename\")" "$store_file"; then
    echo "error: migration $basename exists but is not referenced in run_migrations ($store_file)" >&2
    exit_code=1
  fi
done

if [ "$exit_code" -ne 0 ]; then
  echo "Hint: add the missing migration(s) to the run_migrations array in $store_file" >&2
  exit 1
fi

echo "Verified: all migrations are wired into run_migrations."
