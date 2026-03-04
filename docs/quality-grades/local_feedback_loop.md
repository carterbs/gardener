## Local Feedback Loop Assessment

### Repo-Wide Score: 82
Local validation is strong and CI-reproducible: `.github/workflows/ci.yml` calls `./scripts/run-validate.sh`, and coverage is enforced via `./scripts/test-gardener-coverage.sh`. The main drag is loop speed: pre-commit always runs full coverage (`cargo llvm-cov --all-targets`), and there is no first-class fast/watch test workflow for small edits.

### Per-Domain Scores
- runtime-orchestration: 81 - Runtime entrypoints are explicit (`cargo run -p gardener --bin gardener -- ...`) and test/lint/coverage paths are clear, but default verification cost is high for tight iteration.
- integration-and-contract-testing: 84 - `tools/gardener/tests/` has broad suite coverage and strong fixture-based checks, with good parity to CI scripts, but no documented lightweight “changed-scope” test path.
- developer-automation-and-fixtures: 80 - Automation scripts are comprehensive (`run-validate.sh`, script fixture tests, hook setup), but command surface is fragmented and not unified behind a single task runner target set.

### Key Findings
- CI-to-local parity is excellent: workflows invoke the same scripts developers run locally.
- Pre-commit is robust and deterministic: `.githooks/pre-commit` formats staged Rust files and then runs canonical validation.
- Validation is comprehensive but heavy: `scripts/run-validate.sh` runs multiple custom linters plus full coverage on each full path.
- There is no root `Makefile`/`justfile`, so command discovery relies on docs and script familiarity.

### Deficiencies

- **[FeedbackLoopGap | P1] Commit-time loop is too heavy by default**
  - What: `.githooks/pre-commit` always executes `scripts/run-validate.sh`, which always ends with `scripts/test-gardener-coverage.sh` (`cargo llvm-cov -p gardener --all-targets`).
  - Agent impact: Small fixes incur long wait cycles, slowing hypothesis-test-fix iterations and increasing wasted turns.
  - Fix: Split into `validate-fast` (format/lint/targeted tests) and `validate-full` (coverage/all-targets), use fast tier in pre-commit, keep full tier required in CI.

- **[MissingTooling | P2] No unified task-runner entrypoint**
  - What: No root `Makefile`/`justfile`; validation logic is spread across several scripts under `scripts/`.
  - Agent impact: Agents can run partial/non-canonical command sequences, leading to inconsistent local results vs CI.
  - Fix: Add a thin `justfile` or `Makefile` with canonical targets (`hooks`, `lint`, `test`, `validate-fast`, `validate-full`, `coverage`) delegating to existing scripts.

- **[MissingDocumentation | P2] Quick local loop guidance is incomplete**
  - What: `README.md` and `docs/conventions/workflow.md` clearly document full validation, but not a concise “small change” command matrix for rapid reruns.
  - Agent impact: Agents default to heavyweight checks even when narrower verification is sufficient, reducing throughput.
  - Fix: Add a “Quick Local Loop” section with concrete scoped commands (single test file, test name filter, targeted clippy) and when to escalate to full validation.