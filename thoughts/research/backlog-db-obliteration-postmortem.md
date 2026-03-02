# Postmortem: The Backlog Database Obliteration Bug

**Date:** 2026-03-01
**Severity:** Critical — silent data loss, all in-progress tasks destroyed
**Status:** Fixed

---

## The Symptom

The backlog database at `~/.gardener/backlog.sqlite` was 0 bytes. All 6 queued tasks
were gone. Four open pull requests had no corresponding backlog state. The system
appeared healthy from the outside — the previous Gardener run had exited with
`exit_code: 0`.

---

## Investigation Chain

### Step 1: Audit log shows a file appearing, not being truncated

The watchdog (`~/.gardener/audit.log`) recorded a single entry:

```
2026-03-01T20:22:38 [CHANGE] size=0 (was -1) exists=True mtime=1772414558.04
2026-03-01T20:22:38 [lsof]
COMMAND  PID        USER   FD  TYPE DEVICE SIZE/OFF    NODE NAME
Python  63630  bradcarter   4r   DIR   1,14      160 34776766 /Users/bradcarter/.gardener
```

The `was -1` is the watchdog's `size_bytes` sentinel for "file didn't exist." This
means `backlog.sqlite` wasn't *truncated* — it was freshly *created* at 0 bytes.
Only the watchdog Python process had the directory open at the time; whatever
created the file had already exited.

### Step 2: The audit log itself is missing startup entries

The watchdog was installed at 17:34. The audit log only contains this one entry
from 20:22:38 — no `[watchdog] started` or `[watchdog] baseline` messages that
the watchdog writes at startup. Those lines were written to an *earlier version*
of `audit.log` that no longer exists.

Something had wiped `audit.log` since 17:34.

### Step 3: The OTEL log starts mid-run

`~/.gardener/otel-logs.jsonl` begins with:

```json
{"event_type":"process.exit","payload":{"exit_code":0,"handle":96,...}}
```

Handle 96. On the very first line. Process handles are assigned sequentially
per-process-lifetime; the counter was at 96 before this log file was created.
That means the log file was recreated *during an ongoing run*, not at startup.
The first ~95 child processes spawned by the run are not in the log.

Something had also wiped `otel-logs.jsonl` mid-run.

### Step 4: The DB shows `exists: false` while operations succeed

The backlog write-command events targeting `~/.gardener/backlog.sqlite` all carry
`"main": {"meta": {"exists": false}}` in their `path_state`. Yet the operations
(mark_complete, claim_next, etc.) were succeeding.

This is classic Unix unlink-while-open behavior: the file had been deleted from
the directory namespace, but SQLite still had an open file descriptor to the
inode. Reads and writes continued normally. When the run ended at 20:22:09 and
SQLite closed its connection, the inode was freed — the data was gone for good.

### Step 5: Three files in the same directory, all wiped at different times

At this point the pattern was clear:
- `otel-logs.jsonl` → wiped mid-run (file restarted after handle 96)
- `backlog.sqlite` → deleted while SQLite had it open
- `audit.log` → wiped between 17:34 and 20:22:38

All three files live in `~/.gardener/`. Something was sweeping that directory
and deleting files indiscriminately.

### Step 6: `enforce_total_budget` — the culprit

In `logging.rs`, `append_json` (called on every single OTEL log write) ended with:

```rust
if let Some(parent) = self.path.parent() {
    let _ = enforce_total_budget(parent, self.budget_bytes)?;
}
```

And `enforce_total_budget` in `log_retention.rs`:

```rust
pub fn enforce_total_budget(dir: &Path, budget_bytes: u64) -> Result<Vec<PathBuf>, GardenerError> {
    let mut files = fs::read_dir(dir)
        // ...
        .filter(|path| path.is_file())  // ← every file. no exceptions.
        .collect::<Vec<_>>();

    files.sort_by(|a, b| { /* oldest mtime first */ });

    // delete until total <= budget
    for path in files {
        if total <= budget_bytes { break; }
        fs::remove_file(&path)?;  // ← no logging, no protection
        // ...
    }
}
```

The budget was 50 MB. The function listed **all files** in `~/.gardener/`,
sorted by modification time (oldest first), and deleted until under budget.
It had zero awareness of what the files were. `backlog.sqlite`, `audit.log`,
`otel-logs.jsonl` — all equal candidates for deletion.

When the OTEL log grew large enough that the directory total crossed 50 MB,
`backlog.sqlite` (the oldest file by mtime, since SQLite had last written to it
when the task queue drained) was deleted first. The log was then pruned or
re-created, and the cycle continued for `audit.log`.

The deletion happened via `fs::remove_file` directly — not through the
`runtime.FileSystem` trait that has `backlog.fs.remove.*` guards — so it left
no OTEL trace of its own. The database vanished silently.

### Step 7: The 0-byte file 29 seconds later

After the run completed at 20:22:09, something tried to open the DB again at
20:22:38 (likely a new Gardener session or a re-invocation). `BacklogStore::open`
calls `Connection::open(&path)`, which — for a non-existent path — creates the
file immediately, before any migrations run. If the process then fails or exits
before the first write transaction commits, the file is left at exactly 0 bytes.

The watchdog saw this creation (`size=0, was -1`) and wrote the only entry that
now remains in `audit.log`.

---

## Root Cause

**`enforce_total_budget` treated the backlog database as an expendable log file.**

It ran on `~/.gardener/` — a directory that contains both the OTEL log (which
*should* be prunable) and the backlog database (which is the system's only
durable state). The function sorted all files by mtime and deleted oldest-first
with no allowlist, no type filtering, and no OTEL trace of what it removed.

Secondary contributing factors:
- The 50 MB budget was evaluated against the *entire directory*, not just log
  files, so a large OTEL log would push the directory over budget and trigger
  deletion of the DB.
- `enforce_total_budget` bypassed the `runtime.FileSystem` trait's backlog-path
  guards, so the deletion was invisible in telemetry.
- SQLite's unlink-while-open semantics made the deletion invisible to the
  application until process exit, masking the problem until all data was gone.

---

## Fix (committed)

Three changes:

**1. `log_retention.rs` — protect SQLite files from budget enforcement**

Added an `is_protected` predicate that matches any filename containing `.sqlite`.
`enforce_total_budget` now skips protected files entirely.

**2. `log_retention.rs` — replace budget enforcement with structured rotation**

Added `rotate_log_if_needed(log_path, max_bytes, keep)`:
- When the active log file exceeds `max_bytes` (20 MB), it is renamed to
  `otel-logs.1.jsonl`.
- Existing rotations are shifted up: `.1` → `.2`, `.2` → `.3`.
- The oldest rotation (`.{keep}`) is deleted when the limit is reached.
- Maximum log storage: 4 files × 20 MB = 80 MB. All of it is log files.
- The database is never touched.

**3. `logging.rs` — wire in rotation, remove budget enforcement call**

`append_json` now calls `rotate_log_if_needed` before writing, so the triggering
event lands in the fresh file. `enforce_total_budget` is no longer called on
every write.

The instrumentation linter was updated to exclude `log_retention.rs` — it is
logging infrastructure itself, and calling `append_run_log` from within it while
holding the write lock would deadlock.

---

## What to Check After This Fix

1. **Restore the backlog** — the in-memory state from the last run is gone. The
   4 open PRs (#34, #38, #48, #50) need backlog entries re-created manually.

2. **Verify rotation in production** — after the next long run, check that
   `~/.gardener/otel-logs.1.jsonl` appears and `backlog.sqlite` survives.

3. **Update the log-debugging skill** — it currently reads only `otel-logs.jsonl`.
   It should also check `otel-logs.1.jsonl` etc. when searching for events from
   older runs.

---

## How the Investigation Was Done

The key insight came from reading three signals together:

| Signal | What it told us |
|--------|----------------|
| `audit.log`: `size=0 (was -1)` | File was *created*, not truncated |
| `audit.log`: missing startup entries | `audit.log` itself had been wiped |
| `otel-logs.jsonl`: starts with `process.exit {handle:96}` | Log file was re-created mid-run |

Three different files in the same directory, all wiped. One thing sweeps a
directory: the budget enforcer. `enforce_total_budget` was the only code that
touched `parent_dir` of the log path, and it was called on every write.
