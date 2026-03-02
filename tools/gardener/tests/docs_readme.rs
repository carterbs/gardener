use std::path::{Path, PathBuf};

fn repo_root_path(path: &str) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join(path)
}

#[test]
fn startup_artifact_map_runbook_has_steering_focus() {
    let runbook_path = repo_root_path("../../docs/runbooks/startup-artifact-map.md");
    let runbook = std::fs::read_to_string(&runbook_path)
        .unwrap_or_else(|err| panic!("unable to read startup artifact map runbook: {err}"));

    assert!(
        runbook.contains("# Startup Artifact Map Runbook for Agent Steering"),
        "startup artifact map runbook must include the expected heading"
    );
    assert!(
        runbook.contains("## Startup artifact map"),
        "startup artifact map runbook must include artifact table section"
    );
    assert!(
        runbook.contains("## Steering-specific interpretation"),
        "startup artifact map runbook must include steering interpretation"
    );
    assert!(
        runbook.contains("agent_steering"),
        "startup artifact map runbook should describe agent_steering signals"
    );
}

#[test]
fn docs_readme_uses_stable_worktree_link_target() {
    let readme_path = repo_root_path("../../docs/README.md");
    let readme = std::fs::read_to_string(&readme_path)
        .unwrap_or_else(|err| panic!("unable to read docs/README.md: {err}"));

    assert!(
        !readme.contains("../thoughts/shared/plans/"),
        "docs/README.md must not link to ephemeral thoughts plans"
    );

    assert!(
        readme.contains("[Triage and worktree workflow docs](./conventions/workflow.md)"),
        "docs/README.md must link triage and worktree workflow docs to stable target"
    );

    assert!(
        repo_root_path("../../docs/conventions/workflow.md").exists(),
        "stable triage/worktree workflow docs target is missing"
    );
}

#[test]
fn docs_readme_links_runtime_failure_triage_cookbook() {
    let readme_path = repo_root_path("../../docs/README.md");
    let readme = std::fs::read_to_string(&readme_path)
        .unwrap_or_else(|err| panic!("unable to read docs/README.md: {err}"));

    assert!(
        readme.contains(
            "[OTEL JSONL runtime failure triage cookbook](./runtime-failure-otel-jsonl-cookbook.md)"
        ),
        "docs/README.md must link to the OTEL runtime failure triage cookbook"
    );

    let cookbook_path = repo_root_path("../../docs/runtime-failure-otel-jsonl-cookbook.md");
    assert!(
        cookbook_path.exists(),
        "runtime failure cookbook link points to a missing file"
    );

    let cookbook = std::fs::read_to_string(&cookbook_path)
        .unwrap_or_else(|err| panic!("unable to read runtime failure cookbook: {err}"));
    assert!(
        cookbook.contains("# OTEL JSONL Triage Cookbook for Runtime Failures"),
        "runtime failure cookbook should have expected heading"
    );
    assert!(
        cookbook.contains("## 8) One-command run audit"),
        "runtime failure cookbook should include the one-command run audit"
    );
}

#[test]
fn docs_readme_links_agent_bootstrap_runbook() {
    let readme_path = repo_root_path("../../docs/README.md");
    let readme = std::fs::read_to_string(&readme_path)
        .unwrap_or_else(|err| panic!("unable to read docs/README.md: {err}"));

    let link = "[Agent bootstrap runbook (first run)](./runbooks/agent-bootstrap.md)";
    assert!(
        readme.contains(link),
        "docs/README.md must link to the agent bootstrap runbook"
    );

    let runbook_path = repo_root_path("../../docs/runbooks/agent-bootstrap.md");
    assert!(
        runbook_path.exists(),
        "agent bootstrap runbook link points to a missing file"
    );
}

#[test]
fn agent_bootstrap_runbook_has_required_sections() {
    let runbook_path = repo_root_path("../../docs/runbooks/agent-bootstrap.md");
    let runbook = std::fs::read_to_string(&runbook_path)
        .unwrap_or_else(|err| panic!("unable to read agent bootstrap runbook: {err}"));

    assert!(
        runbook.contains("# Agent Bootstrap Runbook for First-Run Worktree Setup"),
        "agent bootstrap runbook should include the expected heading"
    );
    assert!(
        runbook.contains("## Prerequisites"),
        "agent bootstrap runbook should list prerequisites"
    );
    assert!(
        runbook.contains("## Bootstrap sequence"),
        "agent bootstrap runbook should list bootstrap sequence steps"
    );
    assert!(
        runbook.contains("## Recovery and escalation"),
        "agent bootstrap runbook should include recovery guidance"
    );
    assert!(
        runbook.contains("## Completion criteria"),
        "agent bootstrap runbook should include completion criteria"
    );
}
