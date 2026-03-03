# Quality Grade Report

**Languages**: Rust, Shell, TypeScript
**Overall Readiness**: C+ (72.0)
**Primary Gap**: Untested runtime orchestration boundaries (especially entrypoints and phase handoffs) are the highest-impact gap because they allow startup and lifecycle regressions to escape despite strong module-level tests.

## Agent Readiness

| Dimension | Score | Grade |
|---|---|---|
| Agent Steering | 56 | C- |
| Mechanical Guardrails | 84 | B |
| Local Feedback Loop | 78 | B- |
| Coverage Infrastructure | 64 | C |
| Documentation Quality | 78 | B- |

### Dimension Details

**Agent Steering** (56 / C-): Steering is concise and gives directly usable runtime invocation commands, which is a strong baseline. The score is held down by missing verification command matrices, sparse architecture pointers, and minimal `scripts/` guidance. That forces agents into discovery work before they can run the fastest safe validation loop. The result is lower autonomy efficiency despite high signal-to-noise docs.

**Mechanical Guardrails** (84 / B): Mechanical checks are robust overall: CI validation exists, pre-commit is substantive, coverage gates are active, and custom guardrails are meaningful. This provides strong baseline protection against common regressions. The score is not higher because security scanning and shell lint/format standardization are missing, and some style checks rely too much on local hooks. Those gaps leave policy and portability risks under-detected.

**Local Feedback Loop** (78 / B-): Local workflows are reproducible and largely CI-parity, which is a major strength. Iteration speed is limited by a heavy full-gate path and lack of first-class quick-mode/task-runner ergonomics. Agents therefore pay high per-iteration cost and may reduce check frequency between edits. Better tiering and command ergonomics would materially improve convergence speed.

**Coverage Infrastructure** (64 / C): Rust coverage measurement and threshold gating are present and enforced, which is significant infrastructure maturity. Confidence is reduced by broad ignore scopes and lack of durable artifact/trend visibility. Script-domain coverage is largely uninstrumented, so one domain has strong quantitative controls while the other does not. This unevenness keeps overall coverage infrastructure in the mid range.

**Documentation Quality** (78 / B-): Operational docs, workflows, and runbooks are clear and practical, especially for runtime execution and validation flow. The weak point is code-facing architecture/API documentation in complex runtime modules, where missing rustdoc raises orientation cost. Documentation quality is also reduced by absent docs enforcement tooling and fragmented script references. Overall quality is good but not yet high-assurance for autonomous maintenance.

## Domain Coverage

| Domain | Languages | Coverage | Quality | Risk | Convention | Composite | Grade |
|---|---|---|---|---|---|---|---|
| developer-validation-tooling | Shell | 42 | 89 | 83 | 76 | 50.2 | D |
| runtime-orchestration | Rust | 90 | 94 | 68 | 88 | 76.0 | B- |

### Domain Score Details

**developer-validation-tooling** (50.2 / D)

- **Coverage**: The scripts domain appears under-tested directly, with limited clear evidence of comprehensive script-level suites. That creates weak regression protection for guardrail automation changes.
- **Quality**: Where sampled, validation behavior appears structured and purposeful, suggesting solid baseline quality practices. Confidence is still lower than runtime because fewer rich edge-case script tests were evident.
- **Risk**: Compared with runtime, complexity is lower and exposure is more about operational drift than deep branching failures. The score remains below elite because missing direct script coverage can hide breakages until later.
- **Convention**: Tooling and script organization are coherent, with useful custom validation checks in place. The score is reduced by absent `shellcheck`/`shfmt` enforcement and fragmented script-convention documentation.

**runtime-orchestration** (76.0 / B-)

- **Coverage**: Coverage is strong in `tools/gardener` with extensive module tests and many integration tests, but e2e breadth is still narrow. This leaves full lifecycle sequencing risk underrepresented.
- **Quality**: Tests are behavior-specific and assert meaningful invariants, especially in TUI state machines and rendering flows. The score is reduced by limited direct exercise of live terminal failure and lifecycle paths.
- **Risk**: Complexity is concentrated in oversized core modules and several orchestration boundary files are untested. That combination increases blast radius and makes runtime regressions more likely to escape.
- **Convention**: Runtime conventions are well structured and backed by real guardrails, including strict clippy and convention-linter tests. Adherence is discounted by enforcement drift around `expect_used` and incomplete shell standards coverage.

## Structural Deficiencies

### P0 --- Critical

- **[coverage-gap]** runtime-orchestration: Add boundary integration tests for runtime entrypoints
  - Critical runtime boundary files such as `main.rs`, `phase_cli.rs`, `config.rs`, `merge_loop.rs`, and related phase entry points are insufficiently tested.
  - Without coverage on startup and phase handoff boundaries, agents can pass local unit checks while still breaking real orchestration sequencing and runtime boot behavior.
  - *Remediation*: Create focused integration suites for CLI entry, startup config resolution, phase transitions, and merge loop behavior. Include both happy-path and fault-path assertions so boundary regressions fail early before end-to-end runs.
### P1 --- Important

- **[convention-violation]** runtime-orchestration: Refactor oversized runtime modules by responsibility
  - Core runtime has oversized, high-complexity modules (`tui.rs`, `worker_pool.rs`, `backlog_store.rs`) that mix multiple responsibilities.
  - Agents making localized changes face higher side-effect risk and need more turns to reason safely across dense branching logic.
  - *Remediation*: Split parsing, rendering, input handling, and lifecycle control into narrower modules with cleaner interfaces. Add CI complexity/size checks to prevent regression into new god files.
- **[convention-violation]** runtime-orchestration: Align clippy enforcement with declared workspace policy
  - Declared clippy policy denies `expect_used`, but validation scripts currently allow it during checks.
  - This inconsistency gives agents conflicting signals and allows convention violations to pass automation unexpectedly.
  - *Remediation*: Remove `-A clippy::expect_used` from script-based lint execution or explicitly scope any allowed exceptions in code/tests. Ensure policy and enforcement paths are identical across local and CI checks.
- **[coverage-gap]** runtime-orchestration: Expand deterministic orchestration e2e scenarios
  - End-to-end runtime coverage is narrow, with only a limited hotkey PTY scenario represented.
  - Agents can still ship sequencing or lifecycle failures across startup, triage, execution, and render phases because cross-phase behavior is not broadly validated in realistic run flows.
  - *Remediation*: Add 2-3 deterministic e2e fixtures that execute the Rust entrypoint through full lifecycle paths, including failure modes. Assert phase transitions, outputs, and recovery behavior so lifecycle regressions are caught as integration drift.
- **[coverage-gap]** developer-validation-tooling: Create explicit script contract integration tests
  - Script and validation-tooling coverage is not explicitly comprehensive, and script contracts are not consistently tested as first-class interfaces.
  - Agents changing automation scripts can introduce regressions in migration wiring and guardrails that surface late in CI or production workflows.
  - *Remediation*: For each key script, add fixture-driven tests asserting exit code, stderr/stdout contract, and negative-case behavior. Keep scenarios deterministic so local and CI outcomes match without manual interpretation.
- **[coverage-gap]** developer-validation-tooling: Add scripts-domain operational guidance
  - The `scripts/` domain lacks explicit agent steering for purpose, entrypoints, and expected inputs/outputs.
  - Without direct guidance, agents discover script workflows by trial and error, increasing execution mistakes and missed guardrail runs.
  - *Remediation*: Create `scripts` steering docs listing each primary script, invocation examples, expected outputs, and failure interpretation. Link this from root agent docs so script work has a reliable navigation path.
- **[coverage-gap]** runtime-orchestration: Tighten ignore manifest and enforce path thresholds
  - Coverage gating is concentrated in one Rust surface with an extensive ignore manifest that excludes major runtime paths.
  - Agents can change excluded behavior and still pass coverage checks, creating false confidence in automated merges.
  - *Remediation*: Reduce ignore entries to true exceptions and introduce per-path minimums for protected runtime areas. Fail CI when any protected path drops below threshold even if global line coverage passes.
- **[feedback-loop-gap]** runtime-orchestration: Add live terminal fault-injection tests
  - Live terminal lifecycle paths are lightly tested compared with test-backend rendering, including raw mode toggling, resize/autoresize, and teardown.
  - This weakens confidence in real TTY behavior and causes agent changes to pass tests while still failing in CI/headless or interactive environments.
  - *Remediation*: Introduce injectable terminal/IO abstractions for `with_live_terminal`, `draw`, size queries, and alternate-screen transitions. Write tests that force failure on setup, resize, and cleanup, then assert robust error propagation and terminal restoration.
- **[feedback-loop-gap]** runtime-orchestration: Strengthen TUI buffer-structure assertions
  - Rendering tests rely heavily on string-presence checks rather than structural buffer invariants.
  - Agents can introduce clipping, overflow, or panel placement regressions that still pass because expected text appears somewhere in the frame.
  - *Remediation*: Build reusable assertions for panel boundaries, row counts, ordering, truncation, and viewport behavior in ratatui buffers. Use deterministic fixtures to validate layout semantics rather than just text containment.
- **[feedback-loop-gap]** Enforce rustfmt in CI validation
  - Rust formatting is enforced locally via hook but not as a guaranteed CI gate.
  - If local hooks are absent or bypassed, agents can land style drift that causes noisy follow-up changes and review friction.
  - *Remediation*: Add `cargo fmt --all --check` to the canonical validation script or CI workflow. Keep local hooks for speed, but rely on server-side enforcement for consistency.
- **[feedback-loop-gap]** Implement tiered quick/full validation modes
  - Validation flow is effectively full-gate only, with heavy checks always on the main path.
  - Agents are incentivized to run fewer local checks between edits due to high iteration cost, which increases risky change batching.
  - *Remediation*: Add `--quick` and `--full` modes to validation scripts with clear guarantees and acceptable usage boundaries. Keep pre-commit/CI on full mode while enabling fast local loops for focused edits.
- **[missing-documentation]** runtime-orchestration: Add module/API rustdoc on runtime boundaries
  - Core runtime modules expose sparse module-level and API rustdoc, especially around lifecycle and state-transition boundaries.
  - Agents must reverse-engineer invariants from implementation, increasing edit risk and investigation time in complex orchestration code.
  - *Remediation*: Write `//!` docs for major runtime modules and `///` docs for key public types/functions governing config precedence, FSM transitions, and worker lifecycle. Prioritize high-churn or high-complexity modules first.
- **[missing-tooling]** Document canonical verification command matrix
  - Agent steering documentation provides runtime commands but lacks a verification command matrix for test/lint/format/check workflows.
  - Agents spend extra discovery turns selecting commands and may skip relevant checks, reducing trust in autonomous validation.
  - *Remediation*: Add a compact section with exact copy-paste commands for build, unit/integration tests, formatting, linting, coverage, and migration-wiring checks. Include quick vs full validation guidance to support faster safe loops.
- **[missing-tooling]** developer-validation-tooling: Introduce script-domain coverage measurement
  - There is no dedicated coverage instrumentation or threshold gate for `scripts/` automation.
  - Agents refactoring validation scripts lack quantitative regression signals, so correctness relies too heavily on ad hoc behavior checks.
  - *Remediation*: Adopt shell coverage tooling for key scripts and emit report artifacts alongside Rust coverage. Enforce a minimum threshold for script coverage in CI to keep automation quality measurable.
- **[missing-tooling]** Add required security scanning jobs
  - Security/dependency scanning is not part of required CI guardrails.
  - Agents can introduce vulnerable or policy-disallowed dependencies while still passing functional checks, shifting security detection later in delivery.
  - *Remediation*: Add CI jobs for `cargo audit` and `cargo deny` on pull requests and scheduled runs. Consider CodeQL/secret scanning integration and make security checks required for merge.
- **[missing-tooling]** Add a task runner for validation workflows
  - Local validation lacks a first-class quick task runner and command aliases for common loops.
  - Agents must reconstruct long commands repeatedly, which increases friction and reduces validation frequency during iterative edits.
  - *Remediation*: Introduce a `justfile` or `Makefile` with canonical targets like `quick`, `test`, `lint`, `validate`, and `coverage`. Standardize these targets in docs so local and CI usage stays aligned.
- **[missing-tooling]** Enforce docs build and phase-in missing-doc lint
  - Documentation generation and missing-doc checks are not enforced in validation workflows.
  - Agents can land interface changes that silently degrade maintainability because doc regressions do not fail CI.
  - *Remediation*: Add `cargo doc --no-deps` to validation and introduce `missing_docs` gradually (warn, then deny) for high-value modules. Track exceptions explicitly so enforcement remains intentional.
- **[observability-gap]** Publish coverage artifacts and trends in CI
  - Coverage reporting is summary-only with no durable artifacts or trend surface.
  - Agents cannot quickly inspect which files regressed after failures, which slows diagnosis and increases rerun churn.
  - *Remediation*: Generate LCOV/HTML outputs in CI and upload them as artifacts for every run. Optionally integrate diff-aware coverage reporting to annotate pull requests with module-level deltas.
### P2 --- Nice to Have

- **[convention-violation]** Declare canonical cross-tool steering policy
  - Tool-specific steering files do not clearly define canonical source-of-truth and sync policy.
  - Agents across tools can drift in behavior when parallel docs evolve independently.
  - *Remediation*: State explicitly that `AGENTS.md` is canonical and tool-specific files are thin pointers. Add a maintenance note requiring mirrored updates whenever steering content changes.
- **[convention-violation]** Decouple optional tooling from core preflight
  - Preflight validation requires extra tooling such as `gh` even when unrelated checks are being run.
  - Agents in minimally provisioned environments can be blocked from local validation and pushed into slower CI-only feedback loops.
  - *Remediation*: Split preflight dependencies into required core tools and optional stage-specific tools. Gate `gh` and similar dependencies only for checks that actually need them.
- **[coverage-gap]** runtime-orchestration: Add malformed-input and property tests for normalizers
  - Normalization and parsing helpers lack robust adversarial/property-style coverage for malformed upstream tokens and unexpected shapes.
  - Agents modifying adapters or state mapping can silently degrade queue/state interpretation without immediate detection, producing wrong operational decisions.
  - *Remediation*: Create table-driven malformed-input matrices for backlog/merge parsing and breadcrumb formatting. Add proptest coverage for idempotence and never-panic guarantees on normalization utilities.
- **[coverage-gap]** developer-validation-tooling: Create centralized scripts operations index
  - Script/tooling documentation is fragmented across runbooks and per-script usage text without a single operational index.
  - Agents spend extra turns locating the right script and may choose incomplete validation paths.
  - *Remediation*: Add a `scripts/README.md` documenting each script’s purpose, inputs, outputs, and failure modes. Link it from `docs/README.md` and workflow docs so discovery is deterministic.
- **[feedback-loop-gap]** runtime-orchestration: Add full quality-pipeline golden tests
  - Quality pipeline testing appears stronger at module level than at full-pipeline semantic output level.
  - Agents may produce structurally valid but semantically wrong grading/evidence outputs if cross-module drift is not checked with golden expectations.
  - *Remediation*: Introduce end-to-end fixture tests that execute combined `quality_*` flow and assert stable grade/evidence bundles. Include threshold edge cases and drift checks to detect semantic regressions early.
- **[feedback-loop-gap]** runtime-orchestration: Add runtime architecture map with change guidance
  - Runtime architecture narrative is shallow at the code-facing level despite strong process docs.
  - Agents have weaker orientation for where to implement startup, worker FSM, grading, and adapter changes, increasing wrong-layer edits.
  - *Remediation*: Publish a focused architecture document mapping control flow and module responsibilities, including extension points. Include a 'where to change what' section for common modification intents.
- **[missing-documentation]** Publish concise runtime architecture pointers
  - Architecture pointers for core runtime modules and test locations are minimal in steering docs.
  - Agents lose time mapping ownership boundaries and are more likely to modify the wrong layer for cross-cutting changes.
  - *Remediation*: Add a short map of high-value modules (startup, adapters, worker FSM, grading, CLI boundaries) and where corresponding tests live. Keep one-line responsibilities for each path to reduce navigation errors.
- **[missing-documentation]** Document fast-loop decision matrix
  - Documentation does not include a concise edit-type to command playbook for fastest safe local validation.
  - Agents either over-test and slow down or under-test and miss regressions because command selection lacks explicit decision rules.
  - *Remediation*: Add a short section mapping common change types (Rust unit, integration, scripts-only, docs-only) to minimal safe commands. Include escalation rules for when full validation is required.
- **[missing-documentation]** Add a concise conventions contract to agent docs
  - Steering docs are minimal relative to enforced conventions and validation expectations.
  - Agents must infer standards from scripts and tests instead of one authoritative contract, which slows onboarding and increases inconsistency.
  - *Remediation*: Document lint expectations, validation requirements, and drift-check conventions in one compact section. Link directly to canonical commands and enforcement scripts for quick execution.
- **[missing-tooling]** developer-validation-tooling: Add shell lint and format gates
  - Shell scripts are not consistently linted/formatted with standard tools such as `shellcheck` and `shfmt`.
  - Agents can introduce quoting and portability defects that pass fixtures but fail in different environments.
  - *Remediation*: Integrate `shellcheck` and `shfmt -d` into local and CI validation flows. Provide a documented autofix command for script contributors to resolve findings quickly.
- **[observability-gap]** runtime-orchestration: Replace panic rendering paths with structured errors
  - Some rendering paths use panic-oriented failure behavior rather than structured errors.
  - When failures occur, agents lose actionable diagnostics and recovery context, increasing triage time and retry cycles.
  - *Remediation*: Convert panic/unwrap-based rendering failures into `Result` propagation with typed error metadata. Log stage, terminal dimensions, and panel context to make root-cause analysis deterministic.

## Domain Notes

- **developer-validation-tooling**: Fixture-driven script validation exists and contributes useful guardrail behavior checks.
- **developer-validation-tooling**: Direct, explicit test coverage of `scripts/` contracts is comparatively thin and lowers confidence.
- **developer-validation-tooling**: Risk is lower than runtime due to smaller complexity surface, but script/fixture drift remains a practical failure mode.
- **developer-validation-tooling**: Conventions are decent but lack standard shell lint/format gates that would prevent portability and quoting issues.
- **runtime-orchestration**: Integration and unit coverage is broad across phases, adapters, TUI, CLI, and hotkey paths.
- **runtime-orchestration**: Test depth is high in state and rendering logic, with strong edge-case assertions.
- **runtime-orchestration**: Risk remains elevated due to untested runtime boundary modules and several very large high-branch files.
- **runtime-orchestration**: Conventions are generally strong, but lint-policy mismatch and shell-tooling gaps can weaken enforcement consistency.

---
*Generated: 2026-03-03T21:57:04Z | TTL: 7 days | Assessed by: agent*
