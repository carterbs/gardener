# Backlog

- `2026-03-03-decompose-tui.md`: still open. The code is still in one large `tui.rs` file and there is no `tools/gardener/src/tui/` module tree.
- `2026-03-03-decompose-backlog-store.md`: still open. `backlog_store.rs` is still the monolith rather than a decomposed module set.
- `2026-03-03-decompose-worker-pool.md`: still open. `worker_pool.rs` remains the single large implementation file.
- `2026-02-27-tui-integration-testing.md`: likely not completed as planned. The plan expects `tests/fixtures/agent-responses/...` and separate integration test files, but the current tree still centers these checks inside `tui.rs` and I did not find the fixture tree.
- `2026-02-27-adversarial-tui-tests.md`: likely not completed as planned. The doc proposes dedicated adversarial test files, but I only found the plan itself plus existing in-file tests/adaptor tests, not the new standalone test layout.
