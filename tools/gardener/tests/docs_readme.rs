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
        readme.contains("[Workflow conventions](./conventions/workflow.md)"),
        "docs/README.md must link to workflow conventions"
    );
    assert!(
        readme.contains("[Repository map](./repository-map.md)"),
        "docs/README.md must link to the repository map"
    );

    for required in [
        "../../AGENTS.md",
        "../../README.md",
        "../../docs/quality-grades.md",
        "../../docs/conventions/workflow.md",
        "../../docs/repository-map.md",
    ] {
        assert!(
            repo_root_path(required).exists(),
            "docs/README.md links to non-existent path {required}"
        );
    }

    let repository_map_path = repo_root_path("../../docs/repository-map.md");
    let repository_map = std::fs::read_to_string(&repository_map_path)
        .unwrap_or_else(|err| panic!("unable to read docs/repository-map.md: {err}"));
    assert!(
        repository_map.contains("## Family 1: Orchestration Runtime"),
        "repository map should define the first family section"
    );
    assert!(
        repository_map.contains("## Family 5: Root configuration and metadata"),
        "repository map should include all declared family sections"
    );
    for required in [
        "tools/gardener/src/",
        "scripts/",
        "plans/",
        ".codex/skills/",
        "thoughts/",
    ] {
        assert!(
            repository_map.contains(required),
            "repository map should include family coverage for {required}"
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
