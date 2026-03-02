use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn gitignore_covers_runtime_leftovers() {
    let repo_root = repo_root_path();
    let gitignore = fs::read_to_string(repo_root.join(".gitignore"))
        .unwrap_or_else(|err| panic!("failed to read .gitignore: {err}"));

    let ignore_patterns: Vec<String> = gitignore
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();

    let expected = [
        ".DS_STORE",
        ".DS_Store",
        "*.profraw",
        "default_*.profraw",
        "otel-logs.jsonl",
        "startup-diagnostics/",
        "startup-diagnostics/*.md",
    ];
    for pattern in expected {
        assert!(
            ignore_patterns.contains(&pattern.to_string()),
            "expected .gitignore to include `{pattern}` to avoid runtime artifacts"
        );
    }
}

fn repo_root_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crate should live under <repo_root>/tools/gardener")
        .to_path_buf()
}
