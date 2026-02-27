## Phase 8: TUI + Logging + Disk Budget Controls
Context: [Vision](./00-gardener-vision.md) | [Shared Foundation](./01-shared-foundation.md)
### Changes Required
- Add terminal UI layer:
  - `tools/gardener/src/tui.rs`.
  - use `ratatui` with `crossterm` backend (non-TTY fallback remains structured logs).
- Add event log layer:
  - `tools/gardener/src/logging.rs`
  - `tools/gardener/src/log_retention.rs`.
- Add automated TUI harness for coverage:
  - `FakeTerminal` + deterministic frame capture API
  - `tools/gardener/tests/tui_harness_test.rs` exercises render/interaction branches under test.
- Display requirements:
  - each worker current state
  - friendly tool-call lines
  - state-transition breadcrumbs
  - queue stats (ready/active/failed, counts by priority).
- Add Problems view:
  - classify workers as `healthy` / `stalled` / `zombie` based on heartbeat + session age
  - show current owner/session and blocking reason
  - expose one-key interventions (`retry`, `release lease`, `park/escalate`).
  - thresholds (normative):
    - `healthy`: last heartbeat <= `2 * heartbeat_interval_seconds`
    - `stalled`: last heartbeat > `2 * heartbeat_interval_seconds` and <= `lease_timeout_seconds`
    - `zombie`: last heartbeat > `lease_timeout_seconds` or process/session missing while lease is held.
- Interaction requirements:
  - `q` / `Ctrl-C` graceful shutdown
  - focused worker view
  - scrollable event log.
- Logging requirements:
  - JSONL per run
  - configurable truncation for very large payloads
  - rotation + total-size pruning (<=50MB default budget)
  - non-TTY fallback to plain structured logs.

### Success Criteria
- TUI remains readable with N workers under heavy tool-call volume.
- JSONL traces are useful for postmortem while honoring disk cap.
- Graceful shutdown cancels workers and restores terminal state cleanly.
- Non-TTY fallback mode is deterministic and test-covered.
- Problems view classifications and interventions are deterministic and audit-logged.
- Automated TUI harness covers render/interaction branches; manual TTY smoke is supplemental UX verification.

### Phase Validation Gate (Mandatory)
- Run: `cargo test -p gardener --all-targets`
- Run: `cargo llvm-cov -p gardener --all-targets --summary-only` (must report 100.00% lines for current `tools/gardener/src/**` code at this phase).
- Run E2E binary smoke:
  - non-TTY: `scripts/brad-gardener --target 1 --config tools/gardener/tests/fixtures/configs/phase08-ui.toml > /tmp/gardener-phase8.log`
  - TTY/manual smoke (supplemental): launch `scripts/brad-gardener --target 1 --config tools/gardener/tests/fixtures/configs/phase08-ui.toml` and verify graceful quit (`q` / `Ctrl-C`) and terminal restoration.

### Autonomous Completion Rule
- Continue directly to the next phase only after all success criteria and this phase validation gate pass.
- Do not wait for manual approval checkpoints.
