# Refactor: Split `worker.rs` (2612 lines) into `worker/` module directory

## Context

`tools/gardener/src/worker.rs` is 2612 lines with 6 distinct responsibility clusters interleaved. It contains the doing-worker pipeline, merge-phase orchestration, stream event formatting, evidence/artifact persistence, worktree naming, and simulated test mode — all in one file. Breaking it into a `worker/` directory module with focused submodules improves navigability without changing any public API.

## Public API (unchanged)

Only two consumers import from `crate::worker`:
- `worker_pool.rs`: `execute_merge_phase`, `execute_task`, `worktree_branch_for`, `worktree_path_for`, `MergeRequest`, `WorkerOutcome`, `WorkerStreamEvent`, `WorkerRunSummary`
- `lib.rs`: `worker::worktree_branch_for`

All re-exports stay in `worker/mod.rs` — zero changes to consumers.

## Target structure

```
src/worker/
├── mod.rs                (~40 lines)   re-exports only
├── types.rs              (~90 lines)   WorkerLogEvent, TeardownReport, WorkerRunSummary,
│                                       MergeRequest, WorkerOutcome, WorkerStreamEvent,
│                                       PROMPT_LINE_COMMAND_LIMIT
├── stream_events.rs     (~180 lines)   emit_adapter_tool_event, extract_payload_command,
│                                       format_adapter_event_command, truncate_utf8,
│                                       worker_state_details, extract_failure_reason,
│                                       emit_worker_activity_state[_with],
│                                       merge_polling_block_reason
├── evidence.rs          (~180 lines)   HandoffRunEvidenceBundle, ReviewArtifact,
│                                       collect_handoff_evidence_bundle,
│                                       log_and_persist_review_output,
│                                       review_artifact_path, log_event_from
├── worktree_naming.rs    (~70 lines)   worktree_branch_for, worktree_path_for,
│                                       sanitize_for_branch, worktree_slug_for_task,
│                                       worktree_slug_suffix, WORKTREE_TASK_SLUG_PREFIX_CHARS
├── worker_doing.rs      (~620 lines)   execute_task (dispatcher), execute_task_live
│                                       (understand→planning→doing→gitting→PR→reviewing→handoff)
├── merge_phase.rs       (~735 lines)   execute_merge_phase, worker_merge_main_and_push,
│                                       run_repo_validation_with_quality_guard,
│                                       teardown_after_completion
└── simulated.rs         (~140 lines)   execute_task_simulated
```

## Steps

### Phase 1: Create the directory and mod.rs

1. `mkdir src/worker`
2. `mv src/worker.rs src/worker/mod.rs` (git-preserves history)
3. Verify `cargo check` passes — the module system resolves `worker/mod.rs` identically to `worker.rs`

### Phase 2: Extract submodules (one at a time, cargo check between each)

Extract in dependency order (leaves first):

1. **`worktree_naming.rs`** — pure functions, no internal deps
   - Move: `worktree_branch_for`, `worktree_path_for`, `sanitize_for_branch`, `worktree_slug_for_task`, `worktree_slug_suffix`, `WORKTREE_TASK_SLUG_PREFIX_CHARS`
   - Move tests: `sanitize_for_branch_*`, `worktree_names_are_git_safe_*`, `worktree_slug_for_task_*`

2. **`types.rs`** — all pub types
   - Move: `WorkerLogEvent`, `TeardownReport`, `WorkerRunSummary`, `MergeRequest`, `WorkerOutcome`, `WorkerStreamEvent`, `PROMPT_LINE_COMMAND_LIMIT`
   - Keep `MAX_GITTING_REMEDIATION` in `worker_doing.rs` (it's only used there; the duplicate in `git_phase.rs` is a separate concern)

3. **`stream_events.rs`** — event formatting and emission
   - Move: `emit_adapter_tool_event`, `extract_payload_command`, `format_adapter_event_command`, `truncate_utf8`, `worker_state_details`, `extract_failure_reason`, `emit_worker_activity_state`, `emit_worker_activity_state_with`, `merge_polling_block_reason`
   - Move test: `extract_failure_reason_*`

4. **`evidence.rs`** — artifact persistence
   - Move: `HandoffRunEvidenceBundle`, `ReviewArtifact`, `handoff_evidence_bundle_path`, `collect_handoff_evidence_bundle`, `log_and_persist_review_output`, `review_artifact_path`, `log_event_from`
   - Move tests: `collect_handoff_evidence_bundle_*`, `review_artifact_path_*`

5. **`simulated.rs`** — test-mode path
   - Move: `execute_task_simulated`

6. **`merge_phase.rs`** — merge-and-teardown orchestration
   - Move: `execute_merge_phase`, `worker_merge_main_and_push`, `run_repo_validation_with_quality_guard`, `teardown_after_completion`
   - Move test: `execute_merge_phase_blocks_merge_*`

7. **`worker_doing.rs`** — doing-worker pipeline
   - Move: `execute_task` (dispatcher), `execute_task_live`, `MAX_GITTING_REMEDIATION`
   - Move tests: `worker_executes_fsm_and_teardown_protocol`, `classify_build_and_implement_*`
   - `execute_task` dispatches to `simulated::execute_task_simulated` for test mode

8. **`mod.rs`** — wire up submodules and re-exports:
   ```rust
   mod evidence;
   mod merge_phase;
   mod simulated;
   mod stream_events;
   mod types;
   mod worker_doing;
   mod worktree_naming;

   pub use types::{MergeRequest, WorkerOutcome, WorkerRunSummary, WorkerStreamEvent};
   pub(crate) use merge_phase::execute_merge_phase;
   pub(crate) use worker_doing::execute_task;
   pub(crate) use worktree_naming::{worktree_branch_for, worktree_path_for};
   ```

### Phase 3: Remaining tests that test other modules' functions

Some tests in worker.rs actually test functions from `do_phase`, `review_phase`, `understand_phase` (e.g. `parse_reviewing_output_*`, `parse_understand_output_*`, `parse_doing_output_*`, `fallback_commit_message_*`). These should move to their respective module test blocks, not stay in worker/:
- `parse_reviewing_output_defaults_to_approve_without_verdict` → `review_phase.rs`
- `parse_reviewing_output_preserves_needs_changes_and_suggestions` → `review_phase.rs`
- `parse_understand_output_falls_back_to_classifier` → `understand_phase.rs`
- `parse_doing_output_*` (3 tests) → `do_phase.rs`
- `fallback_commit_message_handles_empty_summary` → `do_phase.rs`
- `classify_build_and_implement_as_feature_for_planning` → `understand_phase.rs`

## Verification

```bash
cargo check -p gardener         # compilation
cargo test -p gardener           # all existing tests pass
cargo clippy -p gardener         # no new warnings
```

No behavioral changes — purely a file reorganization with re-exports.
