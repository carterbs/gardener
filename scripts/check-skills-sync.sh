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
tmp_diff=$(mktemp)
trap 'rm -f "$tmp_claude" "$tmp_codex" "$tmp_diff"' EXIT

find "$claude_skills_dir" -type f -print | sed "s#^${claude_skills_dir}/##" | sort > "$tmp_claude"
find "$codex_skills_dir" -type f -print | sed "s#^${codex_skills_dir}/##" | sort > "$tmp_codex"

missing_in_codex=$(comm -23 "$tmp_claude" "$tmp_codex" || true)
missing_in_claude=$(comm -13 "$tmp_claude" "$tmp_codex" || true)
common_files=$(comm -12 "$tmp_claude" "$tmp_codex" || true)

> "$tmp_diff"
if [[ -n "$common_files" ]]; then
  while IFS= read -r rel_path; do
    if ! cmp -s "$claude_skills_dir/$rel_path" "$codex_skills_dir/$rel_path"; then
      echo "$rel_path" >> "$tmp_diff"
    fi
  done <<< "$common_files"
fi

different_files=$(cat "$tmp_diff")

if [[ -n "$missing_in_codex" || -n "$missing_in_claude" ]]; then
  echo "error: .claude/skills and .codex/skills are out of sync." >&2
  echo "Hint: copy files from one tree to the other, then re-run pre-commit." >&2

  if [[ -n "$missing_in_codex" ]]; then
    echo ".codex/skills is missing:" >&2
    echo "$missing_in_codex" | sed 's/^/  - /' >&2
    echo "Fix suggestion (from .claude -> .codex):" >&2
    echo "$missing_in_codex" | while IFS= read -r rel_path; do
      echo "  mkdir -p \"$(dirname "$codex_skills_dir/$rel_path")\"" >&2
      echo "  cp -f \"$claude_skills_dir/$rel_path\" \"$codex_skills_dir/$rel_path\"" >&2
    done
  fi

  if [[ -n "$missing_in_claude" ]]; then
    echo ".claude/skills is missing:" >&2
    echo "$missing_in_claude" | sed 's/^/  - /' >&2
    echo "Fix suggestion (from .codex -> .claude):" >&2
    echo "$missing_in_claude" | while IFS= read -r rel_path; do
      echo "  mkdir -p \"$(dirname "$claude_skills_dir/$rel_path")\"" >&2
      echo "  cp -f \"$codex_skills_dir/$rel_path\" \"$claude_skills_dir/$rel_path\"" >&2
    done
  fi

  if [[ -n "$different_files" ]]; then
    echo "Matching-path files differ in content:" >&2
    echo "$different_files" | sed 's/^/  - /' >&2
    echo "Fix suggestion (sync .codex from .claude):" >&2
    echo "$different_files" | while IFS= read -r rel_path; do
      echo "  cp -f \"$claude_skills_dir/$rel_path\" \"$codex_skills_dir/$rel_path\"" >&2
    done
  fi

  exit 1
fi

if [[ -n "$different_files" ]]; then
  echo "error: .claude/skills and .codex/skills contain the same file set but differ in content." >&2
  echo "Matching-path files differ in content:" >&2
  echo "$different_files" | sed 's/^/  - /' >&2
  echo "Fix suggestion (sync .codex from .claude):" >&2
  echo "$different_files" | while IFS= read -r rel_path; do
    echo "  cp -f \"$claude_skills_dir/$rel_path\" \"$codex_skills_dir/$rel_path\"" >&2
  done
  exit 1
fi

echo "Verified: .claude/skills and .codex/skills are in sync."
