# Refactor: Split `backlog_store.rs` (2818 lines) into `backlog_store/` module directory

## Context

`tools/gardener/src/backlog_store.rs` is 2818 lines containing the SQLite-backed task persistence layer. It mixes struct definitions, async-channel write dispatch, public API methods with logging boilerplate, raw SQL operations, schema migrations, a read connection pool, diagnostic helpers, and a 580-line test suite. Breaking it into a `backlog_store/` directory module with focused submodules improves navigability and reduces merge conflicts when multiple agents touch different concerns.

## Current state analysis

### Major sections and line ranges

| Lines | Section | Description |
|-------|---------|-------------|
| 1-18 | Imports + constants | `use` statements, `READ_POOL_SIZE`, `StoreResult<T>` type alias |
| 20-56 | `TaskStatus` enum + impls | 7-variant enum with `as_str()` and `from_db()` |
| 58-97 | Data structs | `BacklogTask` (16 fields), `NewTask` (9 fields), `RejectedSeed` (4 fields) |
| 99-186 | `WriteCmd` enum | 16-variant internal command enum for the writer channel |
| 188-204 | `BacklogStore` struct + `Drop` | Struct definition (4 fields) and `Drop` impl |
| 206-644 | `BacklogStore::open()` + helpers | Constructor: directory creation, zero-byte guard, integrity check, migration call, writer thread spawn (300-line write-command dispatch loop), read pool init |
| 646-648 | `db_path()` accessor | Trivial getter |
| 650-1371 | Public API methods | 16 methods that create `WriteCmd` variants, send via channel, log results. Heavy boilerplate. (`upsert_task`, `claim_next`, `mark_in_progress`, `mark_complete`, `release_lease`, `mark_unresolved`, `set_unresolved_to_ready`, `set_unresolved_to_merge_pending`, `clear_related_pr`, `mark_merge_pending`, `claim_merge_pending`, `set_related_pr`, `promote_ready_with_pr`, `recover_stale_leases`, `list_tasks`, `list_backlog_tasks`, `count_tasks_by_priority`, `count_active_tasks`, `get_task`, `insert_rejected_seed`, `list_rejected_seeds`) |
| 1374-1565 | `write_cmd_details()` + `log_write_result()` | Logging/diagnostics for the writer thread |
| 1567-1607 | `backlog_path_state()` | Public diagnostic function that inspects db/wal/shm/bak files |
| 1609-1639 | `ReadPool` struct + impl | Round-robin read connection pool (30 lines) |
| 1641-1648 | `configure_write_connection()` | WAL mode + synchronous FULL pragma setup |
| 1651-1699 | `run_migrations()` | 5-version migration runner |
| 1701-1768 | `upsert_task()` (free fn) | Raw SQL INSERT ... ON CONFLICT upsert |
| 1770-1839 | `claim_next()` / `claim_next_in_tx()` | Transaction-wrapped claim with priority ordering |
| 1841-2086 | SQL mutation functions | `mark_in_progress`, `mark_complete`, `release_lease`, `mark_unresolved`, `set_unresolved_status`, `clear_related_pr`, `mark_merge_pending`, `claim_merge_pending`, `set_related_pr`, `promote_ready_with_pr`, `recover_stale` |
| 2088-2123 | `recover_stale()` | Two-pass stale lease recovery (PR vs non-PR) |
| 2125-2223 | Serialization helpers | `fetch_task`, `row_to_task`, `task_kind_from_db`, `compute_task_id_from_new_task`, `db_err`, `system_time_unix` |
| 2239-2818 | `#[cfg(test)] mod tests` | 16 test functions, ~580 lines |

### Key observations

1. **The write-command dispatch loop** (lines 285-623) is embedded inside `BacklogStore::open()`. It is a 338-line `match` block that dispatches `WriteCmd` variants to free SQL functions plus inline logging. This is the largest single block in the file.

2. **Public API methods** (lines 650-1371) follow a repetitive pattern: create a `oneshot` channel, construct a `WriteCmd`, send it, await the reply, log the result. Each method is ~30-50 lines of near-identical boilerplate. There are 16 of these.

3. **Free SQL functions** (lines 1701-2123) are pure `Connection -> StoreResult<T>` functions that execute raw SQL. They have no dependency on `BacklogStore` or the channel infrastructure.

4. **The `WriteCmd` enum** (lines 99-186) is tightly coupled to both the dispatch loop and the public API methods. It is internal-only (no `pub`).

5. **Tests** reference both public API (`BacklogStore` methods) and internal helpers (`db_err`, `row_to_task`, `task_kind_from_db`). The `super::` imports would need updating.

## Public API (unchanged)

Consumers import these items from `crate::backlog_store`:

| Item | Used by |
|------|---------|
| `BacklogStore` (struct + all pub methods) | `worker_pool.rs`, `quality_pipeline.rs`, `startup.rs`, `quality_backlog_emitter.rs`, `backlog_snapshot.rs`, `friction_analysis.rs`, `worker/merge_phase.rs`, `bin/friction_analysis.rs` |
| `BacklogTask` | `worker_pool.rs`, `backlog_snapshot.rs` |
| `NewTask` | `worker_pool.rs`, `startup.rs`, `quality_backlog_emitter.rs`, `friction_analysis.rs`, `backlog_snapshot.rs` |
| `TaskStatus` | `worker_pool.rs`, `startup.rs`, `friction_analysis.rs` |
| `system_time_unix` | `lib.rs`, `bin/friction_analysis.rs` |
| `backlog_path_state` (pub(crate)) | `startup.rs` |
| `RejectedSeed` | Not imported externally (only used internally + tests) |

All re-exports stay in `backlog_store/mod.rs` -- zero changes to consumers.

## Proposed file structure

```
src/backlog_store/
├── mod.rs              (~310 lines)  BacklogStore struct, Drop impl, open(),
│                                     writer thread spawn + dispatch loop,
│                                     ReadPool, configure_write_connection,
│                                     re-exports
├── types.rs            (~110 lines)  TaskStatus, BacklogTask, NewTask,
│                                     RejectedSeed, WriteCmd, StoreResult alias
├── api.rs              (~730 lines)  impl BacklogStore public API methods
│                                     (all the channel-send-and-log methods)
├── queries.rs          (~150 lines)  Read-only SQL: fetch_task, row_to_task,
│                                     list queries, count queries,
│                                     task_kind_from_db
├── mutations.rs        (~420 lines)  Write SQL functions: upsert_task,
│                                     claim_next, claim_next_in_tx,
│                                     mark_in_progress, mark_complete,
│                                     release_lease, mark_unresolved,
│                                     set_unresolved_status, clear_related_pr,
│                                     mark_merge_pending, claim_merge_pending,
│                                     set_related_pr, promote_ready_with_pr,
│                                     recover_stale
├── migrations.rs       (~60 lines)   run_migrations()
├── logging.rs          (~240 lines)  write_cmd_details, log_write_result,
│                                     backlog_path_state, system_time_unix,
│                                     compute_task_id_from_new_task, db_err
└── tests.rs            (~580 lines)  All #[cfg(test)] tests
```

**Total: ~2600 lines** across 8 files (slightly less than 2818 due to reduced import duplication). Every file is under 750 lines, with most under 500.

## Detailed file contents

### `mod.rs` (~310 lines) -- Core lifecycle and wiring

**What moves here:**
- `use` statements needed for struct definition and `open()`
- `BacklogStore` struct definition (fields become `pub(super)` so sub-files can access)
- `Drop` impl
- `BacklogStore::open()` method including the writer thread spawn and dispatch loop
- `BacklogStore::worker_join_handle()` and `BacklogStore::sender()` private helpers
- `BacklogStore::db_path()` accessor
- `ReadPool` struct + impl (used only from `open()` and `api.rs` read methods)
- `configure_write_connection()` (called only from `open()`)
- `const READ_POOL_SIZE`

**Submodule declarations and re-exports:**
```rust
mod api;
mod logging;
mod migrations;
mod mutations;
mod queries;
mod types;

#[cfg(test)]
mod tests;

// Public re-exports -- preserves exact existing API surface
pub use types::{BacklogTask, NewTask, RejectedSeed, TaskStatus};
pub use logging::system_time_unix;
pub(crate) use logging::backlog_path_state;

// Internal re-exports for sibling modules
use types::{StoreResult, WriteCmd};
```

**Why this stays in mod.rs:** The `open()` constructor owns the writer thread spawn and the dispatch loop. The dispatch loop is the central hub that calls into `mutations.rs` functions and uses `logging.rs` helpers. It must see `WriteCmd` and all the free SQL functions. Keeping it in mod.rs avoids circular dependency between files and keeps the "wiring" in one place. This is the conceptual "main" of the module.

### `types.rs` (~110 lines) -- Data definitions

**What moves here:**
- `type StoreResult<T>` alias (line 18)
- `TaskStatus` enum + `impl TaskStatus` (lines 20-56)
- `BacklogTask` struct (lines 58-76)
- `NewTask` struct (lines 78-89)
- `RejectedSeed` struct (lines 91-97)
- `WriteCmd` enum (lines 99-186)

**Why grouped together:** These are the core data types that every other submodule needs. `WriteCmd` is internal-only but shared between `mod.rs` (dispatch loop) and `api.rs` (method bodies construct `WriteCmd` variants). Putting all types in one place means other files import from `super::types` and don't need cross-file type juggling.

**Visibility notes:**
- `TaskStatus`, `BacklogTask`, `NewTask`, `RejectedSeed`: `pub` (re-exported from mod.rs)
- `WriteCmd`: `pub(super)` (visible within `backlog_store/` only)
- `StoreResult<T>`: `pub(super)`

### `api.rs` (~730 lines) -- Public `impl BacklogStore` methods

**What moves here:** All public methods that follow the channel-send-and-log pattern:
- `upsert_task` (lines 650-699)
- `claim_next` (lines 701-764)
- `mark_in_progress` (lines 766-803)
- `mark_complete` (lines 805-842)
- `release_lease` (lines 844-881)
- `mark_unresolved` (lines 883-920)
- `set_unresolved_to_ready` (lines 922-958)
- `set_unresolved_to_merge_pending` (lines 960-996)
- `clear_related_pr` (lines 998-1033)
- `mark_merge_pending` (lines 1035-1074)
- `claim_merge_pending` (lines 1076-1108)
- `set_related_pr` (lines 1110-1152)
- `promote_ready_with_pr` (lines 1154-1184)
- `recover_stale_leases` (lines 1186-1221)
- `insert_rejected_seed` (lines 1321-1343)
- Read methods that delegate to `read_pool`: `list_tasks` (1223-1245), `list_backlog_tasks` (1247-1270), `count_tasks_by_priority` (1272-1298), `count_active_tasks` (1300-1315), `get_task` (1317-1319), `list_rejected_seeds` (1345-1371)

**Why this file is largest:** Each public method is 30-50 lines of boilerplate (create channel, send WriteCmd, receive, log). This is the primary candidate for future macro-based deduplication, but that refactor is separate from this structural split. Even at ~730 lines it is well under the 1000-line ceiling and contains only one semantic concern: "the public interface layer."

**Access needs:**
- `use super::{BacklogStore, ReadPool}` -- struct access for `impl BacklogStore`
- `use super::types::{WriteCmd, StoreResult, BacklogTask, NewTask, ...}`
- `use super::queries::{fetch_task, row_to_task}` -- for read methods
- `use super::logging::system_time_unix`
- BacklogStore fields `write_tx`, `read_pool`, `db_path` need `pub(super)` visibility

### `queries.rs` (~150 lines) -- Read-only SQL helpers

**What moves here:**
- `fetch_task()` (lines 2125-2144)
- `row_to_task()` (lines 2146-2196)
- `task_kind_from_db()` (lines 2198-2209)

**Why separated from mutations:** These are pure read-only `&Connection -> Result` functions. They are called from both `api.rs` (read methods like `get_task`, `list_tasks`) and `mod.rs` (the dispatch loop calls `fetch_task` after upsert). Isolating them makes it trivial to find and modify query SQL without scrolling past mutation SQL.

**Visibility:** All `pub(super)` -- used within the module only, never exported.

### `mutations.rs` (~420 lines) -- Write SQL functions

**What moves here:**
- `upsert_task()` free fn (lines 1701-1768) -- the big INSERT ON CONFLICT
- `claim_next()` + `claim_next_in_tx()` (lines 1770-1839)
- `mark_in_progress()` (lines 1841-1864)
- `mark_complete()` (lines 1866-1889)
- `release_lease()` (lines 1891-1914)
- `mark_unresolved()` (lines 1916-1939)
- `set_unresolved_status()` (lines 1941-1964)
- `clear_related_pr()` (lines 1966-1984)
- `mark_merge_pending()` (lines 1986-2009)
- `claim_merge_pending()` (lines 2011-2040)
- `set_related_pr()` (lines 2042-2069)
- `promote_ready_with_pr()` (lines 2071-2086)
- `recover_stale()` (lines 2088-2123)

**Why this grouping:** All 13 functions share the same shape: take a `&Connection` (or `&mut Connection`), execute SQL that modifies rows, return `StoreResult`. They are called exclusively from the writer thread dispatch loop in `mod.rs`. Grouping them means SQL review/changes happen in one file.

**Visibility:** All `pub(super)`.

### `migrations.rs` (~60 lines) -- Schema evolution

**What moves here:**
- `run_migrations()` (lines 1651-1699)

**Why its own file:** Migrations are a distinct concern that changes independently (adding new migration versions). Having them in a dedicated file means adding migration 0006 only touches this file. The function is called once from `BacklogStore::open()`.

**Visibility:** `pub(super)`.

### `logging.rs` (~240 lines) -- Diagnostics and utility functions

**What moves here:**
- `write_cmd_details()` (lines 1374-1531) -- maps `WriteCmd` variants to log payloads
- `log_write_result()` (lines 1533-1565) -- structured logging for write outcomes
- `backlog_path_state()` (lines 1567-1607) -- file metadata diagnostics
- `system_time_unix()` (lines 2225-2237) -- timestamp helper
- `compute_task_id_from_new_task()` (lines 2211-2219) -- identity helper
- `db_err()` (lines 2221-2223) -- error conversion

**Why grouped together:** `write_cmd_details` and `log_write_result` are logging infrastructure used by the dispatch loop. `backlog_path_state` is diagnostic. `system_time_unix` is a utility used pervasively. `compute_task_id_from_new_task` is a thin wrapper used by both `mutations::upsert_task` and `logging::write_cmd_details`. `db_err` is the shared error conversion. These are all "support" functions that don't fit neatly into queries or mutations.

**Visibility:**
- `system_time_unix`: `pub` (re-exported from mod.rs, used externally)
- `backlog_path_state`: `pub(crate)` (re-exported from mod.rs)
- `write_cmd_details`, `log_write_result`: `pub(super)`
- `compute_task_id_from_new_task`, `db_err`: `pub(super)`

### `tests.rs` (~580 lines) -- All unit tests

**What moves here:**
- The entire `#[cfg(test)] mod tests` block (lines 2239-2818)
- The `temp_store()` and `task()` test helpers
- All 16 test functions

**Why its own file:** At 580 lines, the test suite is substantial. Separating tests means the production code files stay focused. Tests that need internal access import via `use super::*` or specific items from sibling modules.

**Access needs:**
- Tests currently reference `super::db_err`, `super::task_kind_from_db`, `super::row_to_task` -- these become `super::logging::db_err`, `super::queries::task_kind_from_db`, `super::queries::row_to_task` (or re-exported in mod.rs for test convenience)

## Migration steps

### Phase 1: Create directory and mod.rs

1. `mkdir src/backlog_store`
2. `mv src/backlog_store.rs src/backlog_store/mod.rs`
3. `cargo check -p gardener` -- verify identical resolution

### Phase 2: Extract submodules (one at a time, `cargo check` between each)

Extract in dependency order (leaves first, most-depended-on last):

**Step 1: `types.rs`** (no internal deps)
- Move `StoreResult`, `TaskStatus`, `BacklogTask`, `NewTask`, `RejectedSeed`, `WriteCmd`
- Add `pub(super)` to `WriteCmd` and `StoreResult`
- In mod.rs: `mod types;` and appropriate `use` imports
- `cargo check`

**Step 2: `logging.rs`** (depends on `types`)
- Move `write_cmd_details`, `log_write_result`, `backlog_path_state`, `system_time_unix`, `compute_task_id_from_new_task`, `db_err`
- In mod.rs: `mod logging;`, add re-exports for `system_time_unix` and `backlog_path_state`
- `cargo check`

**Step 3: `migrations.rs`** (depends on `logging` for `system_time_unix` and `db_err`)
- Move `run_migrations`
- In mod.rs: `mod migrations;`
- `cargo check`

**Step 4: `queries.rs`** (depends on `types` and `logging::db_err`)
- Move `fetch_task`, `row_to_task`, `task_kind_from_db`
- In mod.rs: `mod queries;`
- `cargo check`

**Step 5: `mutations.rs`** (depends on `types`, `logging`, `queries`)
- Move all 13 write SQL functions
- In mod.rs: `mod mutations;`
- `cargo check`

**Step 6: `api.rs`** (depends on `types`, `logging`, `queries`)
- Move all `impl BacklogStore` public API methods (except `open`, `db_path`, `sender`, `worker_join_handle`)
- Make BacklogStore fields `pub(super)`: `write_tx`, `read_pool`, `db_path`
- Also make `ReadPool` and its `with_conn` method `pub(super)` so `api.rs` can call `self.read_pool.with_conn()`
- In mod.rs: `mod api;`
- `cargo check`

**Step 7: `tests.rs`** (depends on everything)
- Move the entire `#[cfg(test)] mod tests` block
- Update `super::` references to point through the new module structure
- In mod.rs: `#[cfg(test)] mod tests;`
- `cargo check && cargo test -p gardener`

### Phase 3: Final verification

```bash
cargo check -p gardener         # compilation
cargo test -p gardener           # all existing tests pass
cargo clippy -p gardener         # no new warnings
```

## Public API preservation

The following re-exports in `mod.rs` maintain the exact same `crate::backlog_store::*` API:

```rust
// Re-exported publicly (used by other crates/modules)
pub use types::{BacklogTask, NewTask, RejectedSeed, TaskStatus};
pub use logging::system_time_unix;

// Re-exported at crate level (pub(crate))
pub(crate) use logging::backlog_path_state;
```

No consumer file needs any change.

## Risk assessment

### Low risk

- **`types.rs` extraction**: Pure data types, zero logic. Straightforward move.
- **`migrations.rs` extraction**: Self-contained function called once. Trivial to extract.
- **`tests.rs` extraction**: Tests are already in their own `mod tests` block. Moving to a file just changes `super::` paths.

### Medium risk

- **`queries.rs` / `mutations.rs` split**: The functions `row_to_task` and `fetch_task` are used from both the dispatch loop (mod.rs) and the API methods (api.rs). This creates a diamond dependency: `mod.rs -> queries`, `api.rs -> queries`. Rust handles this fine since they are in the same crate module, but you need to be careful about `pub(super)` visibility.

- **`api.rs` extraction**: Requires making `BacklogStore` fields `pub(super)`. Currently they are private. This is the most invasive visibility change. The `sender()` helper method also needs to be accessible from `api.rs` -- either make it `pub(super)` or move it to `api.rs`.

- **`ReadPool` visibility**: Currently a private struct. The `list_*` and `count_*` methods in `api.rs` call `self.read_pool.with_conn(...)`. Either `ReadPool` becomes `pub(super)` or these read methods stay in `mod.rs`. Recommendation: make `ReadPool` `pub(super)` -- it is an implementation detail that stays within the module boundary.

### Tight coupling to watch

1. **`WriteCmd` <-> dispatch loop <-> mutation functions**: The `WriteCmd` enum variants, the `match` arms in `open()`, and the free SQL functions form a 1:1:1 mapping. Adding a new command requires touching `types.rs` (variant), `mod.rs` (dispatch arm), `mutations.rs` (SQL function), `api.rs` (public method), and `logging.rs` (`write_cmd_details` arm). This is already the case today -- the split does not make it worse, but it also does not improve it. A future macro-based approach could address this.

2. **`compute_task_id_from_new_task`**: Called from both `mutations::upsert_task` and `logging::write_cmd_details`. Placing it in `logging.rs` means `mutations.rs` imports from `logging`. If this feels wrong, it could alternatively live in a shared helpers section of `mod.rs`, but the coupling is minimal (one function call).

3. **`db_err` helper**: Used in nearly every file. Placing it in `logging.rs` and importing from there is fine, but it could also be promoted to the crate-level `errors` module if desired (separate refactor).

## Future improvements (out of scope)

- **Macro to reduce API boilerplate**: The 16 channel-send-log methods in `api.rs` are nearly identical. A declarative macro could reduce each to ~5 lines. This is a logical follow-up after the structural split.
- **Move `db_err` to `crate::errors`**: It is a generic conversion, not specific to backlog_store.
- **Extract `ReadPool` to a shared module**: If other stores are added, the connection pool could be reused.
