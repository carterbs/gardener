# Worker FSM Resilience Plan

## Overview

Make the worker FSM significantly less brittle by adding deterministic fallbacks, git-based work salvage, and retry logic to the ~34 unguarded error propagation points identified in the error handling audit. Organized into four phases: deterministic fallbacks (cheapest, highest ROI), git-based work salvage, retry/backoff for transient failures, and agent-based recovery turns.

## Current State Analysis

The worker FSM has two execution paths — `execute_task_live` in `worker/worker_doing.rs` and `execute_merge_phase` in `worker/merge_phase.rs`. Both return `Result<_, GardenerError>` and use the `?` operator extensively. Errors propagate in two ways:

1. **Blow-ups**: `?` propagates `GardenerError` → pool halts via `shutdown_error` in `worker_pool.rs:559-755`
2. **Premature surrenders**: `Ok(WorkerRunSummary { final_state: Failed })` returned without attempting recovery

Existing resilience patterns to model after:
- **Gitting remediation loop** (`worker_doing.rs:360-470`): `for attempt in 0..MAX_GITTING_REMEDIATION` with `run_agent_turn` inside
- **Merge remediation loop** (`merge_phase.rs:67-633`): State-driven match on `(Mergeable, MergeStateStatus)` with CI/conflict remediation
- **Doing payload fallback** (`worker_doing.rs:275-321`): `commits_since` → `worktree_is_clean` → fail cascade
- **Understand payload fallback** (`understand_phase.rs:97-119`): `classify_task` keyword classifier always succeeds

## Desired End State

- No single transient failure (network blip, process crash) kills a task that has salvageable work
- Agent terminal failures check git for committed work before giving up
- FSM transition errors (internal invariants) are unreachable in correct code but logged + handled rather than blowing up the pool
- Post-merge validation failures don't mark already-merged tasks as Failed
- All recovery paths emit structured log events (`worker.recovery.*`) for observability

## What We're NOT Doing

- Full task retry at pool level (that's a separate backlog/scheduler concern)
- Changing the FSM state machine itself or adding new states
- Adding recovery for `execute_task_simulated` (test mode)
- Retry budget configuration (hardcode sensible defaults now, make configurable later)

---

## Phase 1: Deterministic Fallbacks (no agent turns, no retries)

The cheapest fixes — pure code changes, no new agent invocations, no new dependencies.

### 1a. Understand terminal failure → keyword classifier fallback

**File**: `worker/worker_doing.rs:159-178`

Currently:
```rust
if understand_result.terminal == AgentTerminal::Failure {
    // ... return Failed
}
```

Change to:
```rust
if understand_result.terminal == AgentTerminal::Failure {
    append_run_log("warn", "worker.recovery.understand_terminal_fallback", json!({...}));
    // Fall through — parse_understand_output already handles invalid payloads
    // via classify_task fallback at understand_phase.rs:105
}
```

The `parse_understand_output` call at line 179 already falls back to `classify_task` for bad payloads. A terminal failure just means the payload is likely empty/null — the same fallback applies. Remove the early return.

### 1b. Planning terminal failure → skip to Doing

**File**: `worker/worker_doing.rs:212-231`

Currently:
```rust
if planning_result.terminal == AgentTerminal::Failure {
    // ... return Failed
}
```

Change to:
```rust
if planning_result.terminal == AgentTerminal::Failure {
    append_run_log("warn", "worker.recovery.planning_terminal_skip", json!({...}));
    // Planning is advisory — proceed to Doing without a plan
}
fsm.transition(WorkerState::Doing)?;
```

### 1c. Post-merge validation failure → Complete with warning

**File**: `worker/merge_phase.rs` (post-merge validation block, search for `post_validation_failed`)

Currently returns `WorkerRunSummary { final_state: Failed }` after the PR is already merged.

Change to:
```rust
if let Err(err) = run_repo_validation_with_quality_guard(...) {
    append_run_log("warn", "worker.recovery.post_merge_validation_failed_but_merged", json!({
        "worker_id": worker_id,
        "task_id": task_id,
        "error": err.to_string(),
        "merge_sha": merge_output.merge_sha
    }));
    // PR is already merged — don't mark as failed. Continue to teardown.
    // The friction analysis will pick this up.
}
```

### 1d. PR creation terminal failure → deterministic `gh pr create` fallback

**File**: `worker/worker_doing.rs:490-492`

Currently:
```rust
if pr_result.terminal == AgentTerminal::Failure {
    return Err(GardenerError::Process("pr creation agent failed".to_string()));
}
```

Change to try deterministic PR creation via the existing `GhClient::create_pr`:
```rust
if pr_result.terminal == AgentTerminal::Failure {
    append_run_log("warn", "worker.recovery.pr_creation_deterministic_fallback", json!({...}));
    let title = fallback_commit_message(task_summary);
    let body = format!("Automated PR for task: {task_summary}");
    match gh.create_pr(&title, &body) {
        Ok((number, url)) => { /* use number */ }
        Err(e) => return Err(GardenerError::Process(format!("pr creation failed (agent + deterministic): {e}")));
    }
}
```

### Success Criteria

- `cargo test` passes
- New unit tests for each fallback path (understand terminal → classify_task, planning terminal → skip, post-merge validation fail → complete)
- Log events contain `worker.recovery.*` keys for monitoring

### Confirmation Gate

Verify all existing tests pass. Run `cargo clippy`. Review log output from test runs to confirm recovery events fire correctly.

---

## Phase 2: Git-Based Work Salvage

Add "check git before giving up" to all agent terminal failures in the Doing phase and adapter-level crashes.

### 2a. Doing terminal failure → salvage committed work

**File**: `worker/worker_doing.rs:255-274`

Currently the doing terminal-failure path returns `Failed` immediately. But the doing-output parse-failure path (lines 275-321) already has a three-tier git salvage cascade.

Extract the salvage logic into a helper and use it for both paths:

```rust
/// Check git state for evidence of work done by the agent.
/// Returns Some(DoingOutput) if salvageable, None if no work found.
fn salvage_doing_work_from_git(
    git: &GitClient,
    pre_doing_sha: &str,
    task_summary: &str,
    worker_id: &str,
    task_id: &str,
) -> Result<Option<DoingOutput>, GardenerError> {
    let commits = git.commits_since(pre_doing_sha).unwrap_or_default();
    if let Some(subject) = commits.into_iter().next() {
        append_run_log("warn", "worker.recovery.doing_salvage_from_commits", json!({...}));
        return Ok(Some(DoingOutput { summary: subject }));
    }
    if !git.worktree_is_clean()? {
        let msg = fallback_commit_message(task_summary);
        git.commit_all(&msg)?;
        append_run_log("warn", "worker.recovery.doing_salvage_from_dirty_worktree", json!({...}));
        return Ok(Some(DoingOutput { summary: msg }));
    }
    Ok(None)
}
```

Then in the terminal-failure path:
```rust
if doing_result.terminal == AgentTerminal::Failure {
    if let Ok(Some(_doing_output)) = salvage_doing_work_from_git(&git, &pre_doing_sha, task_summary, worker_id, task_id) {
        append_run_log("warn", "worker.recovery.doing_terminal_failure_salvaged", json!({...}));
        // Continue to gitting phase — agent failed but work was committed
    } else {
        // Truly no work — fail as before
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed, on_event);
        return Ok(WorkerOutcome::Completed(WorkerRunSummary { ... }));
    }
}
```

### 2b. Review terminal failure → default-approve (PR already created)

**File**: `worker/worker_doing.rs:523-542`

At this point the code is pushed and a PR exists. If the review agent crashes, default to approve:

```rust
if reviewing_result.terminal == AgentTerminal::Failure {
    append_run_log("warn", "worker.recovery.review_terminal_default_approve", json!({
        "worker_id": identity.worker_id,
        "task_id": task_id,
        "pr_number": pr_number,
        "reason": "review agent failed, defaulting to approve since PR exists"
    }));
    // Fall through to approve path — parse_reviewing_output already defaults to Approve
    // for missing/invalid payloads
}
```

Note: `parse_reviewing_output` at `review_phase.rs:107-130` already defaults to `ReviewVerdict::Approve` when the verdict field is missing. So we just need to remove the early return on terminal failure and let it fall through.

### Success Criteria

- `salvage_doing_work_from_git` has unit tests covering: commits exist, dirty worktree, clean worktree
- Doing terminal failure with prior commits → task continues to gitting
- Review terminal failure → task continues to merging
- Existing doing-output parse fallback refactored to use `salvage_doing_work_from_git`

### Confirmation Gate

`cargo test` passes. Manual test with a config that forces agent failure to verify salvage path works end-to-end.

---

## Phase 3: Retry with Backoff for Transient Failures

Add retry logic for GitHub API calls and PR lookup that currently blow up on single failures.

### 3a. Add a `retry_with_backoff` utility

**New file**: `tools/gardener/src/retry.rs`

```rust
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use serde_json::json;
use std::time::Duration;

pub struct RetryConfig {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub operation_name: &'static str,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(15),
            operation_name: "operation",
        }
    }
}

pub fn retry_with_backoff<T, F>(config: &RetryConfig, mut f: F) -> Result<T, GardenerError>
where
    F: FnMut() -> Result<T, GardenerError>,
{
    let mut last_err = None;
    for attempt in 0..config.max_attempts {
        match f() {
            Ok(val) => return Ok(val),
            Err(err) => {
                append_run_log("warn", "retry.attempt_failed", json!({
                    "operation": config.operation_name,
                    "attempt": attempt + 1,
                    "max_attempts": config.max_attempts,
                    "error": err.to_string()
                }));
                last_err = Some(err);
                if attempt + 1 < config.max_attempts {
                    let delay_ms = config.base_delay.as_millis() as u64
                        * (attempt as u64 + 1);
                    let delay = Duration::from_millis(delay_ms.min(config.max_delay.as_millis() as u64));
                    std::thread::sleep(delay);
                }
            }
        }
    }
    Err(last_err.unwrap())
}
```

### 3b. Wrap `gh.find_pr_for_branch` with retry

**File**: `worker/worker_doing.rs:493`

```rust
let (number, _url) = retry_with_backoff(
    &RetryConfig { operation_name: "find_pr_for_branch", ..Default::default() },
    || gh.find_pr_for_branch(&branch),
)?;
```

### 3c. Wrap `gh.poll_mergeability` first call with retry

**File**: `worker/merge_phase.rs` (inside the merge loop)

The `poll_mergeability` call at the top of the merge loop already does internal polling for Unknown/Pending states, but a network-level `GardenerError` from `check_mergeability` propagates up. Wrap the outer call:

```rust
let status = retry_with_backoff(
    &RetryConfig { operation_name: "poll_mergeability", max_attempts: 2, ..Default::default() },
    || gh.poll_mergeability(pr, MERGEABILITY_POLL_MAX, MERGEABILITY_POLL_INTERVAL),
)?;
```

### 3d. Wrap `gh.view_pr` after merge with retry

**File**: `worker/merge_phase.rs` (after successful `merge_pr`)

```rust
let view = retry_with_backoff(
    &RetryConfig { operation_name: "view_pr_after_merge", ..Default::default() },
    || gh.view_pr(pr),
)?;
```

### Success Criteria

- `retry_with_backoff` has unit tests covering: immediate success, success on retry, exhaustion
- GitHub API call sites use retry wrapper
- Log events contain `retry.attempt_failed` with attempt count

### Confirmation Gate

`cargo test` passes. `cargo clippy` clean. The retry module has its own test suite.

---

## Phase 4: Guarding `?` Propagation on FSM Transitions and Agent Turns

Convert the remaining unguarded `?` operators into graceful failures where possible.

### 4a. FSM transition errors → log + graceful fail instead of blow-up

FSM transitions (e.g., `fsm.transition(WorkerState::Gitting)?`) should never fail in correct code — they're internal invariant checks. But if they do, the task should fail gracefully rather than taking down the pool.

**File**: `worker/worker_doing.rs` — all `fsm.transition()?` and `fsm.apply_understand()?` calls

Create a helper:
```rust
fn fsm_transition_or_fail(
    fsm: &mut FsmSnapshot,
    next: WorkerState,
    worker_id: &str,
    task_id: &str,
    identity: &WorkerIdentity,
    logs: &[WorkerLogEvent],
    on_event: Option<&dyn Fn(WorkerStreamEvent)>,
) -> Result<(), WorkerRunSummary> {
    fsm.transition(next).map_err(|err| {
        append_run_log("error", "worker.recovery.fsm_transition_failed", json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "target_state": next.as_str(),
            "error": err.to_string()
        }));
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed, on_event);
        WorkerRunSummary {
            worker_id: identity.worker_id.clone(),
            session_id: identity.session.session_id.clone(),
            final_state: WorkerState::Failed,
            logs: logs.to_vec(),
            teardown: None,
            failure_reason: Some(format!("internal FSM error: {err}")),
        }
    })
}
```

Usage:
```rust
fsm_transition_or_fail(&mut fsm, WorkerState::Gitting, worker_id, task_id, &identity, &logs, on_event)
    .map_err(|summary| return Ok(WorkerOutcome::Completed(summary)))?;
```

Or more idiomatically, use a macro or an early-return pattern.

### 4b. `run_agent_turn()?` → catch adapter crashes, check git

For the Doing phase specifically, wrapping `run_agent_turn` to catch adapter-level crashes:

**File**: `worker/worker_doing.rs:239-253`

```rust
let doing_result = match run_agent_turn(AgentTurnInput { ... }) {
    Ok(result) => result,
    Err(agent_err) => {
        append_run_log("error", "worker.recovery.doing_agent_crash", json!({
            "worker_id": worker_id,
            "task_id": task_id,
            "error": agent_err.to_string()
        }));
        // Agent process crashed — check if it did any work before dying
        if let Ok(Some(salvaged)) = salvage_doing_work_from_git(&git, &pre_doing_sha, task_summary, worker_id, task_id) {
            append_run_log("warn", "worker.recovery.doing_agent_crash_salvaged", json!({...}));
            // Skip payload parsing, go straight to gitting
            let _ = salvaged;
            // ... continue to gitting
        } else {
            emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Failed, on_event);
            return Ok(WorkerOutcome::Completed(WorkerRunSummary {
                worker_id: identity.worker_id,
                session_id: identity.session.session_id,
                final_state: WorkerState::Failed,
                logs,
                teardown: None,
                failure_reason: Some(format!("agent process crashed: {agent_err}")),
            }));
        }
    }
};
```

### 4c. Blocked PR → park instead of fail

**File**: `worker/merge_phase.rs` (Blocked branch with no failed checks)

Currently returns `Failed` immediately. Change to `Parked` so the task can be retried:

```rust
if !has_explicit_failed_checks(&failed_checks) {
    if attempt + 1 >= MAX_MERGE_REMEDIATION {
        emit_worker_activity_state(worker_id, task_id, WorkerActivityState::Parked, on_event);
        return Ok(WorkerRunSummary {
            final_state: WorkerState::Parked,  // was: Failed
            failure_reason: Some("PR blocked by branch protection rules, parked for retry".to_string()),
            ..
        });
    }
    continue;
}
```

### Success Criteria

- FSM transition failures return `Ok(WorkerOutcome::Completed(Failed))` instead of `Err(GardenerError)`
- Doing agent crash with prior commits → task continues
- Blocked PR without failed checks → Parked (not Failed)
- No `GardenerError` from `execute_task_live` or `execute_merge_phase` except for truly unrecoverable infrastructure failures
- All recovery paths have structured log events

### Confirmation Gate

`cargo test` passes. Manual review of all `?` usage in `worker_doing.rs` and `merge_phase.rs` to confirm no premature blow-ups remain for recoverable scenarios.

---

## Testing Strategy

**Unit tests** (per phase):
- `salvage_doing_work_from_git`: commits found, dirty worktree, clean worktree
- `retry_with_backoff`: immediate success, retry success, exhaustion
- FSM transition guard: invalid transition → graceful fail
- Understand fallback: terminal failure → classify_task output used
- Planning skip: terminal failure → Doing state reached

**Integration tests**:
- `FakeProcessRunner` configured to simulate agent terminal failure with git commits present → verify task reaches gitting
- `FakeProcessRunner` configured to simulate adapter crash → verify git salvage
- Post-merge validation failure → verify `final_state == Complete`

**Manual verification**:
- Run gardener with `test_mode = false` against a test repo
- Verify recovery log events appear in OTEL logs
- Verify TUI shows correct state transitions during recovery

## References

- Error audit: conversation context (2026-03-03)
- Existing gitting remediation: `worker/worker_doing.rs:360-470`
- Existing merge remediation: `worker/merge_phase.rs:67-633`
- Existing payload fallbacks: `understand_phase.rs:97-119`, `worker/worker_doing.rs:275-321`
- Pool error dispatch: `worker_pool.rs:559-755`
