use std::path::PathBuf;

#[test]
fn claude_md_redirects_to_agents_md() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root should be two levels above tools/gardener manifest");
    let claude_path = repo_root.join("CLAUDE.md");

    let contents =
        std::fs::read_to_string(&claude_path).expect("CLAUDE.md should be present at repo root");

    assert!(
        contents.contains("[AGENTS.md](./AGENTS.md)"),
        "Expected CLAUDE.md to contain AGENTS.md redirect"
    );
}
