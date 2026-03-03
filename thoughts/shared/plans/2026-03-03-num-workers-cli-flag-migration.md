# Plan: Move CLI worker-count flag to `--num-workers`

## Overview
Rename the runtime CLI override flag from `--parallelism` to `--num-workers`, while preserving a short deprecation window so existing scripts do not break immediately. The config schema (`orchestrator.parallelism`) remains unchanged for now.

## Current State Analysis
- Runtime CLI currently exposes `--parallelism` via `Cli.parallelism` in `tools/gardener/src/lib.rs:90-92`.
- Parsed CLI value is forwarded into config overrides as `CliOverrides.parallelism` in `tools/gardener/src/lib.rs:238-243` and then applied in `tools/gardener/src/config.rs:520-523`.
- Runtime behavior that guards triage profile overrides depends on whether CLI parallelism override is set (`apply_profile_runtime_preferences`) in `tools/gardener/src/lib.rs:819-835`.
- Config load logging emits `parallelism_override` and resolved `parallelism` in `tools/gardener/src/config.rs:295-299` and `tools/gardener/src/config.rs:352-357`.
- Help contract tests explicitly assert `--parallelism` appears in output (`tools/gardener/tests/phase1_contracts.rs:126-130`).
- Scheduler smoke test uses `--parallelism` (`tools/gardener/tests/phase04_scheduler.rs:10-13`).
- Default project config and fixtures continue to use config key `orchestrator.parallelism` (`gardener.toml:4-5` and `tools/gardener/tests/fixtures/configs/phase04-scheduler-stub.toml:4-5`).
- No `--paralallism` token exists in code today; requested spelling appears to be a user typo rather than an existing flag.

## Desired End State
- Primary CLI flag is `--num-workers`.
- `--parallelism` is accepted as a deprecated alias for one release cycle (with warning), then removed.
- Optional typo alias `--paralallism` is either:
  - accepted as hidden alias only during transition, or
  - explicitly rejected with a targeted error suggesting `--num-workers`.
- All CLI/help tests, smoke tests, and runtime docs are updated to match the new flag contract.
- Config file key remains `orchestrator.parallelism` (no TOML migration in this change).

## Key Discoveries
- The CLI override value affects precedence rules with triage profile defaults (`tools/gardener/src/lib.rs:824-833`), so a field rename must preserve the "CLI override disables profile override" behavior.
- `CliOverrides` is shared by multiple binaries (`tools/gardener/src/phase_cli.rs:42-57`, `tools/gardener/src/bin/seed_backlog.rs:72-87`, `tools/gardener/src/bin/friction_analysis.rs:116-131`), so internal field naming should be updated consistently or clearly documented as legacy.
- The strongest regression surface is help text and command-line parsing contracts (`tools/gardener/tests/phase1_contracts.rs:123-145`, `tools/gardener/tests/phase04_scheduler.rs:7-20`).

## What We Are Not Doing
- Renaming config key `orchestrator.parallelism` to `orchestrator.num_workers`.
- Changing worker-pool internal naming (`parallelism`) in scheduler/runtime internals.
- Altering triage profile schema fields (`preferred_parallelism`) in this migration.

## Implementation Approach
Use a CLI-surface migration only:
1. Introduce `--num-workers` as the canonical long flag.
2. Keep compatibility aliases short-term.
3. Update tests/docs to canonicalize on `--num-workers`.
4. Add clear deprecation/remediation messaging.

## Implementation Phases

### Phase 1: CLI Parse Surface Migration
Overview: Make parser accept `--num-workers` while preserving compatibility.

Changes required:
- Update CLI struct field/arg metadata in `tools/gardener/src/lib.rs`.
- Keep existing internal override plumbing intact (or rename with mechanical updates):
  - `CliOverrides` (`tools/gardener/src/config.rs:12-28`)
  - override application (`tools/gardener/src/config.rs:520-523`)
  - profile preference gate (`tools/gardener/src/lib.rs:819-835`)
- Decide compatibility mode:
  - `--parallelism` alias with deprecation warning (recommended),
  - optional hidden `--paralallism` alias or targeted error messaging.

Success criteria:
- `gardener --help` shows `--num-workers`.
- `gardener --num-workers 3` behaves identically to previous `--parallelism 3`.
- Existing automation using `--parallelism` still runs during transition (if alias path chosen).

Confirmation gate:
- Confirm exact deprecation policy: one release-cycle alias vs immediate hard break.

### Phase 2: Test Contract Updates
Overview: Align tests with the new canonical flag and verify backward compatibility path.

Changes required:
- Update help contract expectations in `tools/gardener/tests/phase1_contracts.rs:126-130`.
- Update scheduler smoke invocation in `tools/gardener/tests/phase04_scheduler.rs:10-13`.
- Add explicit parser tests for:
  - canonical `--num-workers`,
  - deprecated `--parallelism` alias behavior,
  - typo handling policy for `--paralallism`.

Success criteria:
- Contract tests pass with new canonical flag.
- Compatibility behavior is covered by deterministic tests.

Confirmation gate:
- Verify tests codify intended user-facing error/warning text.

### Phase 3: Docs and Runtime Messaging
Overview: Ensure documentation and logs reflect the new interface.

Changes required:
- Update runtime usage docs (starting with `README.md`, `AGENTS.md`, and any runbooks/examples that invoke worker-count override).
- If deprecation alias retained, add migration note and removal timeline.
- Keep config docs unchanged unless explicitly expanding scope.

Success criteria:
- No documentation examples prefer `--parallelism`.
- Migration path for existing scripts is clearly documented.

Confirmation gate:
- Validate that all user-facing command snippets use `--num-workers`.

### Phase 4: Alias Removal (Deferred Follow-Up)
Overview: Remove deprecated aliases after announced window.

Changes required:
- Remove `--parallelism` (and optional typo alias) from clap metadata.
- Drop compatibility tests and deprecation warnings.
- Keep only `--num-workers` tests.

Success criteria:
- Parser hard-fails legacy flags with clear remediation.

Confirmation gate:
- Perform only after migration window expires and downstream scripts are updated.

## Testing Strategy
Automated:
- `cargo test -p gardener --test phase1_contracts`
- `cargo test -p gardener --test cli_smoke`
- `cargo test -p gardener --test phase04_scheduler -- --ignored` (if environment allows ignored smoke)
- Full project validation: `./scripts/run-validate.sh`

Manual:
- `cargo run -p gardener --bin gardener -- --help` and confirm `--num-workers` appears.
- `cargo run -p gardener --bin gardener -- --num-workers 1 --quit-after 0 --config tools/gardener/tests/fixtures/configs/phase01-minimal.toml`
- If alias retained, run same command with `--parallelism 1` and confirm expected warning/behavior.

## Risks and Mitigations
- Risk: breaking existing scripts that pass `--parallelism`.
  - Mitigation: temporary alias + deprecation warning + documented sunset date.
- Risk: precedence regressions with triage profile preference.
  - Mitigation: add targeted unit/contract test around `apply_profile_runtime_preferences` behavior.
- Risk: inconsistent docs/tests causing future regressions.
  - Mitigation: update contract tests and docs in same change.

## References
- `tools/gardener/src/lib.rs:82-120`
- `tools/gardener/src/lib.rs:238-253`
- `tools/gardener/src/lib.rs:430-443`
- `tools/gardener/src/lib.rs:819-835`
- `tools/gardener/src/config.rs:12-28`
- `tools/gardener/src/config.rs:285-367`
- `tools/gardener/src/config.rs:520-523`
- `tools/gardener/tests/phase1_contracts.rs:123-145`
- `tools/gardener/tests/phase04_scheduler.rs:7-20`
- `gardener.toml:4-5`
- `tools/gardener/tests/fixtures/configs/phase04-scheduler-stub.toml:4-5`
