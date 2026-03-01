---
date: 2026-03-01
researcher: codex
git_commit: 19b7f83
branch: main
topic: Why `feat: implement task changes` still appears on main and in PR titles
tags: [worker, commit-message, pr-title, regression]
status: complete
---

# Research Question

Why does `feat: implement task changes` keep appearing on `main` even after we added logic to derive PR title/body from commit messages, and where is the regression in the Rust runtime flow?

# Summary

The live Rust worker still creates commits with a hardcoded subject: `feat: implement task changes`. That hardcoded commit subject was introduced by the deterministic commit flow in commit `b0b6e85` and is still present on `main`.

A later fix (`fb4f6c0`) changed PR title/body generation to read commit messages (including body), but it did not change how commit messages are created. Since PR title generation picks the first commit subject from `main..HEAD`, the generic hardcoded commit subject propagates directly into PR titles and then into merge history on `main`.

# Detailed Findings

## Commit message creation is hardcoded in live worker flow

The primary live path commits deterministically using a fixed string:
- `git.commit_all("feat: implement task changes")` in `execute_task_live`.
- This executes after successful Doing completion and before deterministic push/PR creation.

There are also fixed remediation commit subjects in later phases:
- `fix: gitting remediation`
- `fix: merge remediation`

`GitClient::commit_all` itself is generic (`&str`), so the limitation is caller-side, not git client capability.

## PR title/body derivation reads commits, but does not correct generic subjects

`generate_pr_title_body` now reads full commit messages from `git log main..HEAD --reverse --format=%B%x00`.
- Title = first commit subject (oldest commit in range).
- Single commit body = commit description if present, else `task_summary`.
- Multiple commits body = `task_summary` + listed commit entries.

There is no logic to detect or replace generic subjects (e.g. `feat: implement task changes`). Therefore, when the deterministic commit is the first commit, PR title becomes that generic subject.

## Data model supports commit_message but runtime ignores it

`DoingOutput` includes `commit_message`, and prompt templates explicitly require returning it. But the live worker does not pass `DoingOutput.commit_message` into `git.commit_all`; it always uses the hardcoded subject. This explains the mismatch between intended contract and actual behavior.

## Historical timeline

- `b0b6e85` (2026-02-28): introduced deterministic commit flow and hardcoded `feat: implement task changes` in worker.
- `fb4f6c0` (2026-03-01): changed PR title/body generation to derive from commit messages (subject/body), but only touched `gh.rs`.
- Current `main` (`19b7f83`) still contains hardcoded worker commit subject, so regression is active.

# Code References

| File | Lines | Description |
|------|-------|-------------|
| `tools/gardener/src/worker.rs` | 307-311 | Deterministic commit with hardcoded subject `feat: implement task changes`. |
| `tools/gardener/src/worker.rs` | 411 | Hardcoded remediation commit subject `fix: gitting remediation`. |
| `tools/gardener/src/worker.rs` | 640 | Hardcoded remediation commit subject `fix: merge remediation`. |
| `tools/gardener/src/git.rs` | 32-66 | `commit_all(&str)` stages all and commits with provided message. |
| `tools/gardener/src/gh.rs` | 420-479 | PR title/body generation from `main..HEAD` commit messages. |
| `tools/gardener/src/fsm.rs` | 34-38 | `DoingOutput` includes `commit_message`. |
| `tools/gardener/src/prompt_registry.rs` | 143-144, 185-186 | Doing prompt requires `commit_message` in output schema. |

# Architecture Insights

The runtime currently uses a split responsibility that is inconsistent:
- Agent-facing contract collects commit message intent (`DoingOutput.commit_message`).
- Runtime execution path enforces deterministic commit with fixed literals.
- PR metadata generation trusts git history, not task summary, for title.

This creates a deterministic but low-signal commit/PR title pipeline where generic text is preserved end-to-end.

# Historical Context

`git blame` on `tools/gardener/src/worker.rs` line 310 points to `b0b6e85`, confirming the hardcoded subject was introduced by the deterministic gitting/merging refactor. `fb4f6c0` only modified `tools/gardener/src/gh.rs`, so commit generation behavior never changed during the PR title/body improvement.

# Open Questions

- Should runtime consume `DoingOutput.commit_message` directly, or should it sanitize/validate it first and fall back to deterministic defaults?
- Should PR title generation skip known-generic subjects (e.g. `feat: implement task changes`) when selecting a title from commit history?
