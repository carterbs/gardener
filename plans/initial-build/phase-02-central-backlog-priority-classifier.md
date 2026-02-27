## Phase 2: Central Backlog Engine + Priority Classifier
Context: [Vision](./00-gardener-vision.md) | [Shared Foundation](./01-shared-foundation.md)
### Changes Required
- Add backlog store:
  - `tools/gardener/src/backlog_store.rs` (SQLite schema/migrations)
  - `tools/gardener/src/priority.rs`
  - `tools/gardener/src/task_identity.rs`
- Schema includes: `task_id`, `title`, `details`, `priority`, `status`, `last_updated`, `lease_owner`, `lease_expires_at`, `source`, `related_pr`, `related_branch`, `attempt_count`, `created_at`.
- Use WAL mode with:
  - single DB write actor for all mutations/claims
  - read-only connection pool for scheduler/TUI projection reads.
  - write actor implemented as dedicated Tokio task with `mpsc<WriteCmd>` + per-command `oneshot` reply.
- Implement atomic claim via one `UPDATE ... RETURNING` transaction.
- Implement startup crash recovery for stale leases/in-progress tasks.
- Add deterministic classifier that maps detected conditions to `P0/P1/P2`.
- Implement task identity hashing contract from shared foundation (`kind`, normalized `title`, `scope_key`, `related_pr`, `related_branch`).
- Add markdown snapshot exporter (read-only view):
  - `tools/gardener/src/backlog_snapshot.rs`.

### Success Criteria
- Worker lease/ack transitions are atomic and race-safe.
- Priority ordering and FIFO-by-`last_updated` are deterministic.
- Backlog cannot duplicate logically identical tasks due to stable identity hashes.
- Reinsert with same identity upgrades existing row instead of duplicating.
- Two concurrent claim attempts never return the same task.
- Crash recovery reliably requeues abandoned leased tasks.

### Phase Validation Gate (Mandatory)
- Run: `cargo test -p gardener --all-targets`
- Run: `cargo llvm-cov -p gardener --all-targets --summary-only` (must report 100.00% lines for current `tools/gardener/src/**` code at this phase).
- Run E2E binary smoke: `scripts/brad-gardener --backlog-only --config tools/gardener/tests/fixtures/configs/phase02-backlog.toml` then `scripts/brad-gardener --target 1 --config tools/gardener/tests/fixtures/configs/phase02-backlog.toml`.

### Autonomous Completion Rule
- Continue directly to the next phase only after all success criteria and this phase validation gate pass.
- Do not wait for manual approval checkpoints.
