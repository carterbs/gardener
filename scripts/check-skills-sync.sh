#!/usr/bin/env bash
set -euo pipefail

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  cd "$(git rev-parse --show-toplevel)"
fi

claude_skills_dir=".claude/skills"
codex_skills_dir=".codex/skills"

if [[ ! -d "$claude_skills_dir" || ! -d "$codex_skills_dir" ]]; then
  echo "error: both .claude/skills and .codex/skills must exist" >&2
  exit 1
fi

tmp_claude=$(mktemp)
tmp_codex=$(mktemp)
trap 'rm -f "$tmp_claude" "$tmp_codex"' EXIT

find "$claude_skills_dir" -type f -print | sed "s#^${claude_skills_dir}/##" | sort > "$tmp_claude"
find "$codex_skills_dir" -type f -print | sed "s#^${codex_skills_dir}/##" | sort > "$tmp_codex"

missing_in_codex=$(comm -23 "$tmp_claude" "$tmp_codex" || true)
missing_in_claude=$(comm -13 "$tmp_claude" "$tmp_codex" || true)

if [[ -n "$missing_in_codex" || -n "$missing_in_claude" ]]; then
  echo "error: .claude/skills and .codex/skills are out of sync." >&2

  if [[ -n "$missing_in_codex" ]]; then
    echo ".codex/skills is missing:" >&2
    echo "$missing_in_codex" | sed 's/^/  - /' >&2
  fi

  if [[ -n "$missing_in_claude" ]]; then
    echo ".claude/skills is missing:" >&2
    echo "$missing_in_claude" | sed 's/^/  - /' >&2
  fi

  exit 1
fi

echo "Verified: .claude/skills and .codex/skills are in sync."
