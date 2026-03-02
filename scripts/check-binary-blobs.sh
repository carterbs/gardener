#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

declare -a targets=("$@")
if [[ "${#targets[@]}" -eq 0 ]]; then
  while IFS= read -r target; do
    targets+=("$target")
  done < <(git diff --cached --name-only --diff-filter=ACM || true)
fi

if [[ "${#targets[@]}" -eq 0 ]]; then
  echo "Verified: no staged binary blobs detected."
  exit 0
fi

if ! command -v file >/dev/null 2>&1; then
  echo "error: required command 'file' is missing" >&2
  exit 1
fi

blocked=()
for target in "${targets[@]}"; do
  [[ -f "$target" ]] || continue

  case "$target" in
    *.profraw|default_*.profraw|*/startup-diagnostics/*|startup-diagnostics/*)
      blocked+=("$target (known runtime artifact)")
      continue
      ;;
  esac

  mime_type="$(file -b --mime-type "$target")"
  mime_encoding="$(file -b --mime-encoding "$target")"
  if [[ "$mime_encoding" == "binary" ]]; then
    blocked+=("$target")
    continue
  fi

  case "$mime_type" in
    text/* | inode/*)
      if [[ "$mime_encoding" != "utf-8" && "$mime_encoding" != "us-ascii" ]]; then
        blocked+=("$target")
        continue
      fi
      ;;
    application/json | application/xml | application/javascript | application/x-shellscript | application/x-perl | application/x-python*)
      continue
      ;;
    *)
      blocked+=("$target")
      ;;
  esac
done

if [[ "${#blocked[@]}" -gt 0 ]]; then
  echo "error: blocked artifact(s) detected in staged files:" >&2
  for target in "${blocked[@]}"; do
    echo "  - $target" >&2
  done
  echo "Hint: remove binary artifacts from commits or store them outside git." >&2
  exit 1
fi

echo "Verified: no staged binary blobs detected."
