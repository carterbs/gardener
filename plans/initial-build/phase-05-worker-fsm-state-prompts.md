## Phase 5: Worker FSM + State Prompts
Context: [Vision](./00-gardener-vision.md) | [Shared Foundation](./01-shared-foundation.md)
### Changes Required
- Add explicit FSM:
  - `tools/gardener/src/fsm.rs`
  - `tools/gardener/src/worker.rs`
  - `tools/gardener/src/worker_identity.rs`
  - `tools/gardener/src/prompts.rs`
  - `tools/gardener/src/prompt_context.rs`
  - `tools/gardener/src/prompt_registry.rs`
  - `tools/gardener/src/prompt_knowledge.rs`.
- State definitions and transitions:
  - `UNDERSTAND` (task categorization: task/chore/infra/feature/bugfix/refactor)
  - `PLANNING` (only when category requires planning)
  - `DOING` (max 100 turns)
  - `GITTING`
  - `REVIEWING` (suggestions loop max 3 back to `DOING`)
  - `MERGING`
  - `COMPLETE`
  - `FAILED`/`PARKED` terminal handling.
- Add per-state backend+model resolution from config.
- Model worker runtime identity explicitly:
  - stable `worker_id`
  - per-attempt `session_id`
  - per-session `sandbox_id`.
- Add transition validator that rejects illegal transitions at compile-time/runtime boundary.
- Add done-means-gone completion teardown protocol:
  - complete -> merge verification -> session/sandbox teardown -> worktree cleanup -> state clear.
- Implement prompt packet construction contract for every state:
  - deterministic context assembly + ranking + token-budget trimming
  - required packet sections (`task_packet`, `repo_context`, `evidence_context`, `execution_context`, `knowledge_context`)
  - `context_manifest` generation with source hashes and inclusion rationale.
- Bootstrap V1 prompt registry by porting existing `scripts/ralph` prompt content/state intent:
  - preserve behavioral semantics of `UNDERSTAND`, `PLANNING`, `DOING`, `GITTING`, `REVIEWING`, `MERGING`
  - preserve current guardrails and expected output-shape instructions
  - adapt placeholders/context wiring to Rust prompt packet format.
  - remove known fragile prompt patterns during port:
    - ambiguous output formatting instructions
    - missing schema-envelope requirement
    - conflicting merge/validation directives across states.
- Add typed state outputs:
  - `UNDERSTAND`: `{ task_type, reasoning }`
  - `DOING`: `{ summary, files_changed }`
  - `GITTING`: `{ branch, pr_number, pr_url }`
  - `REVIEWING`: `{ verdict, suggestions }`
  - `MERGING`: `{ merged, merge_sha }`.
- Enforce git boundary in FSM implementation:
  - `GITTING`/`MERGING` consume agent-produced git/PR outputs and run deterministic verification only.
  - FSM must not embed broad git orchestration logic beyond invariant checks and escalation hooks.
- Add structured output extraction contract implementation:
  - prompts require final `<<GARDENER_JSON_START>>...<<GARDENER_JSON_END>>` envelope
  - parser takes last complete envelope, validates `schema_version` + `state`, and deserializes typed payload
  - extraction errors map to typed retry/escalation paths.
- Add learning-loop modules:
  - `tools/gardener/src/learning_loop.rs`
  - `tools/gardener/src/postmerge_analysis.rs`
  - `tools/gardener/src/postmortem.rs`
  - knowledge entry scoring/decay and prompt-integration hooks.

### Success Criteria
- FSM is explicit, typed, and transition-safe.
- Turn/cycle limits are enforced exactly.
- Category-driven skip-planning behavior is deterministic.
- Skip-planning category mapping (`task|chore|infra` skip, `feature|bugfix|refactor` require planning) is implemented exactly.
- Illegal transition attempts fail fast and are logged.
- Review-loop cap parks tasks deterministically with actionable failure reason.
- Task completion always tears down session/sandbox/worktree with no leaked bindings.
- `GITTING`/`MERGING` do not regress into orchestrator-driven git scripting; agent remains git operator of record.
- Retry path always creates a fresh session identity.
- Restart recovery preserves worker slot identity and always creates a new session identity with resume linkage.
- Prompt packets are deterministic for identical task/repo snapshots and include required context sections.
- Determinism is proven with fixed ranking sort key and `sha256` manifest hashing contract.
- Every worker step logs `prompt_version` + `context_manifest_hash`.
- Post-merge and failed-task analyzers produce evidence-backed knowledge entries that influence subsequent prompt packets.
- V1 Rust prompt templates are parity-checked against legacy prompt intent with golden fixtures; prompt iteration is deferred to post-cutover phases.

### Phase Validation Gate (Mandatory)
- Run: `cargo test -p gardener --all-targets`
- Run: `cargo llvm-cov -p gardener --all-targets --summary-only` (must report 100.00% lines for current `tools/gardener/src/**` code at this phase).
- Run E2E binary smoke: `scripts/brad-gardener --task "fixture/fsm-basic" --target 1 --config tools/gardener/tests/fixtures/configs/phase05-fsm.toml`.

### Autonomous Completion Rule
- Continue directly to the next phase only after all success criteria and this phase validation gate pass.
- Do not wait for manual approval checkpoints.
