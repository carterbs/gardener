# Test Suite Parallelism Robustness

## Overview

Make the test suite pass reliably under default `cargo test` parallel execution,
eliminating the need for `RUST_TEST_THREADS=1`.

Two root causes were identified: a hardcoded shared SQLite path in integration
tests, and missing `expectrl` timeouts in PTY e2e tests.

---

## Current State Analysis

### Root Cause 1 — Shared SQLite path in `phase1_contracts`

`tools/gardener/tests/phase1_contracts.rs:19`

```rust
const TEST_REPO_ROOT: &str = "/tmp/gardener-phase1-contracts";
```

Three tests in this file share the same on-disk path:
- `run_with_runtime_paths_and_errors` (line 156)
- A non-TTY guard test (line 232)
- `run_with_runtime_propagates_write_and_config_errors` (line 298)

All three create a real SQLite file at
`/tmp/gardener-phase1-contracts/.cache/gardener/backlog.sqlite` (because
`test_mode = true` routes to `<repo_root>/.cache/gardener/backlog.sqlite`).
The "database is locked" failure happens when two of these tests run
concurrently and one holds the write connection while the other tries to open
or migrate the same file.

The manual cleanup at line 158 (`remove_file`) is a workaround for leftover
state, not a fix for the race. Running a second test immediately after
`remove_file` while the first test's write thread is still shutting down still
produces a lock.

### Root Cause 2 — Missing `expectrl` timeout on two PTY tests

`tools/gardener/tests/hotkey_pty_e2e.rs`:

| Test | Line | `set_expect_timeout` set? |
|---|---|---|
| `pty_e2e_hotkeys_v_g_b_q_drive_screen_transitions` | 257 | No |
| `pty_e2e_ctrl_c_quits` | 286 | No |
| `pty_e2e_q_interrupts_live_blocking_turn` | 303 | Yes — 8s |
| `pty_e2e_ctrl_c_interrupts_live_blocking_turn` | 323 | Yes — 8s |

The two tests without an explicit timeout fall back to `expectrl`'s internal
default, which is too tight under parallel CPU load (other tests competing for
the CPU make the TUI startup slower).

### Minor Issue — `temp_store()` leaks temp directories

`tools/gardener/src/backlog_store.rs:1014-1027`

`temp_store()` uses `process::id() + AtomicU64 + nanos` for uniqueness (already
correct and race-free) but never cleans up the directory because it returns only
`BacklogStore`, not a `TempDir` guard. This is not a flakiness cause but is a
test hygiene issue.

---

## Desired End State

- `cargo test -p gardener --all-targets` passes consistently with default
  parallelism (no `RUST_TEST_THREADS=1`)
- No shared mutable paths between concurrently-running tests
- All PTY tests have an explicit `set_expect_timeout` that is generous enough
  for a loaded CI machine
- `temp_store()` does not leak temp directories

---

## What We're NOT Doing

- Not switching to `nextest` or adding a `.cargo/config.toml` parallelism cap —
  fixing root causes is better than capping concurrency
- Not switching PTY tests to `#[serial]` (via the `serial_test` crate) — they
  are not inherently serial once timeouts are correct
- Not moving unit tests to in-memory SQLite — the file-based path exercises real
  WAL behaviour and is worth keeping

---

## Implementation Approach

Fix each root cause surgically with minimal diff.

---

## Phase 1: Fix shared SQLite path in `phase1_contracts`

### Overview

Replace the module-level `const TEST_REPO_ROOT` with per-test `TempDir`
instances so each test gets its own SQLite file.

### Changes required

**File: `tools/gardener/tests/phase1_contracts.rs`**

1. **Add import** (line 15, with existing `use std::path::{Path, PathBuf};`):
   ```rust
   use tempfile::TempDir;
   ```
   (`tempfile` is already a dev-dependency in `tools/gardener/Cargo.toml`)

2. **Delete** the constant at line 19:
   ```rust
   // DELETE THIS:
   const TEST_REPO_ROOT: &str = "/tmp/gardener-phase1-contracts";
   ```

3. **`run_with_runtime_paths_and_errors` (line 156):**
   Replace the stale-file removal + hardcoded path with a `TempDir`:
   ```rust
   #[test]
   fn run_with_runtime_paths_and_errors() {
       let dir = TempDir::new().expect("tempdir");
       let repo_root = dir.path().to_str().expect("utf8").to_string();
       let runtime = runtime_with_config(
           "[execution]\ntest_mode = true\nworker_mode = \"normal\"\n",
           true,
           Some(&repo_root),
       );
       // ... rest of test unchanged
   }
   ```
   Remove the `remove_file` call entirely — `TempDir::new()` always starts fresh.

4. **Non-TTY test at line 232:** Replace `Some(TEST_REPO_ROOT)` with a local dir:
   ```rust
   let dir2 = TempDir::new().expect("tempdir");
   let repo_root2 = dir2.path().to_str().expect("utf8").to_string();
   let non_tty_runtime = runtime_with_config("", false, Some(&repo_root2));
   ```

5. **`run_with_runtime_propagates_write_and_config_errors` at line 298:**
   Replace `Some(TEST_REPO_ROOT)` with a local dir:
   ```rust
   let dir = TempDir::new().expect("tempdir");
   let repo_root = dir.path().to_str().expect("utf8").to_string();
   let ok_runtime = runtime_with_config("", true, Some(&repo_root));
   ```

### Success criteria

- **Automated:** `cargo test -p gardener --all-targets phase1_contracts` passes
  10/10 times when run with `RUST_TEST_THREADS=4`
- **Manual:** Verify no `/tmp/gardener-phase1-contracts` directory is created
  during the test run

### Confirmation gate

Run `for i in $(seq 10); do cargo test -p gardener phase1_contracts; done` and
confirm zero failures before proceeding to Phase 2.

---

## Phase 2: Add explicit timeouts to all PTY tests

### Overview

Add `session.set_expect_timeout(Some(Duration::from_secs(30)))` immediately
after `Session::spawn` in the two tests that currently have no timeout.

30 seconds is generous for a TUI test (the existing 8s tests are for a `sleep 5`
subprocess), safe for CI, and fast enough to not mask real hangs.

### Changes required

**File: `tools/gardener/tests/hotkey_pty_e2e.rs`**

1. **`pty_e2e_hotkeys_v_g_b_q_drive_screen_transitions` (line 259):**
   ```rust
   let mut session = expectrl::Session::spawn(cmd).expect("spawn pty");
   session.set_expect_timeout(Some(Duration::from_secs(30))); // ADD THIS
   ```

2. **`pty_e2e_ctrl_c_quits` (line 289):**
   ```rust
   let mut session = expectrl::Session::spawn(cmd).expect("spawn pty");
   session.set_expect_timeout(Some(Duration::from_secs(30))); // ADD THIS
   ```

### Success criteria

- **Automated:** `cargo test -p gardener --all-targets hotkey_pty_e2e` passes
  5/5 times with default parallelism (no `RUST_TEST_THREADS=1`)
- **Manual:** A deliberately slow machine / loaded CI run does not flake on
  these two tests

### Confirmation gate

Run `cargo test -p gardener hotkey_pty_e2e` 5 times and confirm all pass before
proceeding to Phase 3.

---

## Phase 3: Fix `temp_store()` cleanup

### Overview

Switch `temp_store()` to return `(BacklogStore, TempDir)` so the temp directory
is cleaned up when the test ends. The unique-path logic (process ID + counter)
can be removed since `TempDir` guarantees uniqueness.

### Changes required

**File: `tools/gardener/src/backlog_store.rs`**

1. **Add import** inside `mod tests` (line 1000 area):
   ```rust
   use tempfile::TempDir;
   ```

2. **Replace `temp_store()`** (lines 1012-1027):
   ```rust
   fn temp_store() -> (BacklogStore, TempDir) {
       let dir = TempDir::new().expect("tempdir");
       let db = dir.path().join("backlog.sqlite");
       let store = BacklogStore::open(&db).expect("open store");
       (store, dir)
   }
   ```
   - Remove `static TEST_DB_COUNTER`
   - Remove the nonce/counter/process-id logic

3. **Update all call sites** inside `mod tests`. Each `temp_store()` call
   becomes:
   ```rust
   let (store, _dir) = temp_store();
   ```
   The `_dir` binding keeps `TempDir` alive for the test duration and drops
   (cleaning up) when the binding goes out of scope.

   Grep for call sites:
   ```
   grep -n "temp_store()" tools/gardener/src/backlog_store.rs
   ```

### Success criteria

- **Automated:** `cargo test -p gardener backlog_store` compiles and passes
- **Manual:** `/tmp` does not accumulate `gardener-backlog-*` directories after
  running the unit tests

### Confirmation gate

Run `cargo test -p gardener --lib` and confirm all unit tests pass.

---

## Testing Strategy

### Automated

After all three phases:

```sh
# Run full suite 5× with default parallelism — must be 0 failures
for i in 1 2 3 4 5; do
  cargo test -p gardener --all-targets && echo "PASS $i" || echo "FAIL $i"
done
```

### Manual smoke check

```sh
# Confirm no shared-path artifacts
ls /tmp/gardener-phase1-contracts 2>/dev/null && echo "LEAKED" || echo "CLEAN"
ls /tmp/gardener-backlog-* 2>/dev/null && echo "LEAKED" || echo "CLEAN"
```

---

## References

- Root cause report from agent run (Feb 28 2026)
- `tools/gardener/tests/phase1_contracts.rs:19` — hardcoded shared path
- `tools/gardener/tests/hotkey_pty_e2e.rs:257,286` — missing expectrl timeouts
- `tools/gardener/src/backlog_store.rs:1014` — temp_store cleanup
- `tools/gardener/src/startup.rs:36-58` — `backlog_db_path()` showing how
  `test_mode = true` routes to `<repo_root>/.cache/gardener/backlog.sqlite`
