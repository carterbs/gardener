use std::path::{Path, PathBuf};

fn repo_root_path(path: &str) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join(path)
}

#[test]
fn docs_readme_is_navigation_index() {
    let readme_path = repo_root_path("../../docs/README.md");
    let readme = std::fs::read_to_string(&readme_path)
        .unwrap_or_else(|err| panic!("unable to read docs/README.md: {err}"));

    assert!(
        readme.contains("# Agent Navigation Index"),
        "docs/README.md should declare itself as the navigation index"
    );
    assert!(
        readme.contains("[`AGENTS.md`](../AGENTS.md)"),
        "docs/README.md must link to AGENTS.md"
    );
    assert!(
        readme.contains("[`README.md`](../README.md)"),
        "docs/README.md must link to root README.md"
    );
    assert!(
        readme.contains("[Quality Grades](./quality-grades.md)"),
        "docs/README.md must link to quality grades"
    );
    assert!(
        readme.contains("[Backlog operations runbook](./runbooks/backlog-operations.md)"),
        "docs/README.md must link to the backlog operations runbook"
    );
    assert!(
        readme.contains("[Workflow conventions](./conventions/workflow.md)"),
        "docs/README.md must link to workflow conventions"
    );

    for required in [
        "../../AGENTS.md",
        "../../README.md",
        "../../docs/quality-grades.md",
        "../../docs/conventions/workflow.md",
        "../../docs/runbooks/backlog-operations.md",
    ] {
        assert!(
            repo_root_path(required).exists(),
            "docs/README.md links to non-existent path {required}"
        );
    }

    let readme_dir_listing = readme
        .lines()
        .filter(|line| line.trim_start().starts_with("- ["))
        .count();
    assert!(
        readme_dir_listing >= 8,
        "docs/README.md should provide a minimum set of agent navigation links"
    );
}
