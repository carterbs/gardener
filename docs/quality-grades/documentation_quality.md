## Documentation Quality Assessment

### Repo-Wide Score: 78
The repository has strong operational documentation (root `README.md`, `docs/README.md`, workflow/runbooks, and troubleshooting cookbooks) with concrete commands and clear agent-oriented navigation. The biggest gaps are code-level API/architecture documentation inside core Rust runtime modules and lack of enforced rustdoc generation/missing-doc checks.

### Per-Domain Scores
- runtime-orchestration: 80 - Runtime usage and workflows are well documented (`README.md`, `docs/conventions/workflow.md`, runbooks), but core module/API rustdoc coverage is uneven and module-level architecture docs in code are largely absent.
- developer-validation-tooling: 69 - Validation flow is documented and scripts have CLI usage text, but script/fixture documentation is fragmented and there is no single authoritative script operations reference in `scripts/`.

### Key Findings
- Top-level and operator docs are strong: onboarding, runtime entrypoints, validation pipeline, and failure triage are clearly documented.
- Documentation is more process-focused than code-interface-focused; public Rust runtime surfaces are under-documented for fast autonomous reasoning.
- Doc tooling is not enforced in validation (no `cargo doc`/`missing_docs` policy), so API docs can drift without failing checks.

### Deficiencies

- **[MissingDocumentation | P1]** Sparse rustdoc in core runtime boundaries
  - What: Core files like `tools/gardener/src/lib.rs`, `tools/gardener/src/runtime/mod.rs`, `tools/gardener/src/config.rs`, `tools/gardener/src/fsm.rs`, and `tools/gardener/src/worker_pool.rs` have little to no module-level docs (`//!` count is effectively zero across `tools/gardener/src`).
  - Agent impact: Agents must infer invariants and state transitions from implementation, increasing wrong edits and extra investigation turns in high-complexity orchestration code.
  - Fix: Add `//!` module docs plus targeted `///` docs on key public types/functions (runtime abstractions, FSM transitions, config precedence, worker pool lifecycle).

- **[MissingTooling | P1]** No doc-generation or missing-doc enforcement in validation
  - What: `scripts/run-validate.sh`, workspace `Cargo.toml`, and `tools/gardener/Cargo.toml` do not enforce rustdoc generation or missing-doc linting.
  - Agent impact: Documentation regressions are invisible to CI/pre-commit, so agents can ship interface changes that silently reduce maintainability and safe reuse.
  - Fix: Add a docs stage (`cargo doc --no-deps`) and phase in `missing_docs` (warn then deny) for high-value runtime modules.

- **[CoverageGap | P2]** Script/tooling documentation is scattered
  - What: `developer-validation-tooling` guidance is split across `docs/conventions/workflow.md`, runbooks, and per-script `usage()` text, with no centralized `scripts/README.md` or equivalent index.
  - Agent impact: Agents spend extra turns discovering which script to run for a specific failure mode and may choose incomplete validation paths.
  - Fix: Create a single script operations index documenting each script’s purpose, inputs, outputs, and when to run it; link from `docs/README.md` and workflow docs.

- **[FeedbackLoopGap | P2]** Runtime architecture narrative is shallow at code-facing level
  - What: `docs/repository-map.md` gives high-level families, but there is no concise runtime architecture doc mapping key modules/flows (startup, worker FSM, quality grading, adapters) to extension points.
  - Agent impact: Autonomous agents have slower orientation in `tools/gardener/src` and higher risk of modifying the wrong layer during cross-cutting changes.
  - Fix: Add a focused runtime architecture doc (or section) with module responsibilities, control flow, and “where to change what” guidance, linked from `docs/README.md`.