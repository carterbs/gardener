## Num workers CLI flag migration
Context: `worker-count` → `num-workers` CLI deprecation and compatibility migration.

### Changes Required
- Replace existing CLI surface `--parallelism` usage with `--num-workers`.
- Keep a backward-compatible alias `--worker-count` with conflict detection and deprecation warning.
- Prefer `--num-workers` in docs, phase runbooks, and command examples.
- Add regression coverage for:
  - parsing `--num-workers`.
  - parsing `--worker-count` and emitting deprecation warning.
  - rejecting simultaneous use of both flags.

### Acceptance Criteria
- `cargo run -p gardener --bin gardener -- --help` lists `--num-workers` and `--worker-count`.
- CLI smoke command accepts `--num-workers` in phase-04/coverage examples.
- Unit/integration tests validate new flag parsing and deprecation behavior.
- Existing profile/override precedence remains unchanged (command-line flag still wins).

### Validation Gate
- Run: `cargo test -p gardener --all-targets`
- Run: `cargo clippy -p gardener --all-targets -- -D warnings` *(workspace currently reports pre-existing `clippy::expect-used` failures; no new migration-specific failures expected)*
- Run: `cargo run -p gardener --bin gardener -- --help`
- Run: `cargo run -p gardener --bin gardener -- --num-workers 1 --prune-only --config tools/gardener/tests/fixtures/configs/phase01-minimal.toml`
- Run: `cargo run -p gardener --bin gardener -- --worker-count 1 --prune-only --config tools/gardener/tests/fixtures/configs/phase01-minimal.toml`
