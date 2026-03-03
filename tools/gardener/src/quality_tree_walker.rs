use crate::quality_language_registry::identify_language;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Directories always excluded from scanning.
const DEFAULT_EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "third_party",
    ".git",
    "target",
    "dist",
    "build",
    "out",
    ".build",
    ".next",
    ".cache",
    "__pycache__",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    "venv",
    ".venv",
];

/// File patterns always excluded.
const DEFAULT_EXCLUDED_FILE_PATTERNS: &[&str] = &[
    ".min.js",
    ".min.css",
    ".bundle.js",
    "package-lock.json",
    "Cargo.lock",
    "yarn.lock",
    "Gemfile.lock",
    "poetry.lock",
    "pnpm-lock.yaml",
];

/// Generated file patterns (suffix match).
const DEFAULT_GENERATED_PATTERNS: &[&str] = &[
    ".pb.go",
    ".generated.",
    "_generated.",
    ".gen.",
    "_gen.",
];

/// Source file extensions we care about.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "swift", "py", "pyi", "go",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeWalkerOutput {
    pub directories: Vec<DirectoryEntry>,
    pub language_summary: BTreeMap<String, usize>,
    pub total_source_files: usize,
    pub total_test_files: usize,
    pub excluded_directories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub path: String,
    pub source_files: Vec<FileEntry>,
    pub test_files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub language: String,
    pub signature: Vec<String>,
    pub line_count: usize,
}

/// Walk a repository and enumerate source/test files by directory and language.
pub fn walk_repo(repo_path: &Path) -> TreeWalkerOutput {
    let custom_ignores = load_quality_ignore(repo_path);
    let mut directories: BTreeMap<String, (Vec<FileEntry>, Vec<FileEntry>)> = BTreeMap::new();
    let mut language_summary: BTreeMap<String, usize> = BTreeMap::new();
    let mut excluded_directories: Vec<String> = Vec::new();
    let mut total_source = 0usize;
    let mut total_test = 0usize;

    walk_directory(
        repo_path,
        repo_path,
        &custom_ignores,
        &mut directories,
        &mut language_summary,
        &mut excluded_directories,
        &mut total_source,
        &mut total_test,
    );

    let directories = directories
        .into_iter()
        .map(|(path, (source_files, test_files))| DirectoryEntry {
            path,
            source_files,
            test_files,
        })
        .collect();

    excluded_directories.sort();
    excluded_directories.dedup();

    TreeWalkerOutput {
        directories,
        language_summary,
        total_source_files: total_source,
        total_test_files: total_test,
        excluded_directories,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_directory(
    root: &Path,
    dir: &Path,
    custom_ignores: &[String],
    directories: &mut BTreeMap<String, (Vec<FileEntry>, Vec<FileEntry>)>,
    language_summary: &mut BTreeMap<String, usize>,
    excluded_directories: &mut Vec<String>,
    total_source: &mut usize,
    total_test: &mut usize,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut file_entries: Vec<std::fs::DirEntry> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            if is_excluded_dir(dir_name, &path, root, custom_ignores) {
                if let Ok(rel) = path.strip_prefix(root) {
                    excluded_directories.push(rel.to_string_lossy().to_string());
                }
                continue;
            }

            walk_directory(
                root,
                &path,
                custom_ignores,
                directories,
                language_summary,
                excluded_directories,
                total_source,
                total_test,
            );
        } else {
            file_entries.push(entry);
        }
    }

    for entry in file_entries {
        let path = entry.path();
        if is_excluded_file(&path) {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if !SOURCE_EXTENSIONS.contains(&ext) {
            continue;
        }

        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        let first_line = read_first_line(&path);
        let language = identify_language(&path, first_line.as_deref());
        if language == "Unknown" {
            continue;
        }

        let (signature, line_count) = read_signature(&path);

        let file_entry = FileEntry {
            path: rel_path.clone(),
            language: language.clone(),
            signature,
            line_count,
        };

        let is_test = is_test_file(&path, &rel_path);

        let dir_key = path
            .parent()
            .and_then(|p| p.strip_prefix(root).ok())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let entry = directories.entry(dir_key).or_insert_with(|| (Vec::new(), Vec::new()));
        if is_test {
            entry.1.push(file_entry);
            *total_test += 1;
        } else {
            entry.0.push(file_entry);
            *total_source += 1;
        }

        *language_summary.entry(language).or_insert(0) += 1;
    }
}

fn is_excluded_dir(dir_name: &str, full_path: &Path, root: &Path, custom_ignores: &[String]) -> bool {
    if DEFAULT_EXCLUDED_DIRS.contains(&dir_name) {
        return true;
    }
    // Check custom ignores (simple prefix/contains matching)
    if let Ok(rel) = full_path.strip_prefix(root) {
        let rel_str = rel.to_string_lossy();
        for pattern in custom_ignores {
            let trimmed = pattern.trim_end_matches('/');
            if rel_str == trimmed || rel_str.starts_with(&format!("{trimmed}/")) {
                return true;
            }
        }
    }
    false
}

fn is_excluded_file(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let path_str = path.to_string_lossy();

    for pattern in DEFAULT_EXCLUDED_FILE_PATTERNS {
        if file_name == *pattern || file_name.ends_with(pattern) {
            return true;
        }
    }

    for pattern in DEFAULT_GENERATED_PATTERNS {
        if path_str.contains(pattern) {
            return true;
        }
    }

    false
}

fn is_test_file(path: &Path, rel_path: &str) -> bool {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let lower = file_name.to_ascii_lowercase();
    let rel_lower = rel_path.to_ascii_lowercase();

    // Path-based patterns
    if lower.ends_with("_test.go")
        || lower.starts_with("test_")
        || lower.ends_with("_test.py")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".test.js")
        || lower.ends_with(".spec.js")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".spec.tsx")
        || lower.ends_with("tests.swift")
    {
        return true;
    }

    // Directory conventions
    if rel_lower.contains("__tests__/")
        || rel_lower.contains("/tests/")
        || rel_lower.starts_with("tests/")
        || rel_lower.contains("/test/")
        || rel_lower.starts_with("test/")
    {
        return true;
    }

    false
}

fn read_first_line(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    reader.lines().next()?.ok()
}

/// Read the first 20 non-blank lines as a file signature, plus total line count.
fn read_signature(path: &Path) -> (Vec<String>, usize) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (Vec::new(), 0),
    };
    let reader = BufReader::new(file);
    let mut signature = Vec::new();
    let mut line_count = 0;

    for line in reader.lines().map_while(Result::ok) {
        line_count += 1;
        if signature.len() < 20 && !line.trim().is_empty() {
            signature.push(line);
        }
    }

    (signature, line_count)
}

/// Load `.gardener/quality-ignore` if it exists (gitignore-like syntax, simplified).
fn load_quality_ignore(repo_path: &Path) -> Vec<String> {
    let ignore_path = repo_path.join(".gardener/quality-ignore");
    let content = match std::fs::read_to_string(&ignore_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
}

/// Helper: collect all source file paths from tree walker output.
pub fn all_source_file_paths(output: &TreeWalkerOutput) -> Vec<String> {
    output
        .directories
        .iter()
        .flat_map(|d| d.source_files.iter().map(|f| f.path.clone()))
        .collect()
}

/// Helper: collect all test file paths from tree walker output.
pub fn all_test_file_paths(output: &TreeWalkerOutput) -> Vec<String> {
    output
        .directories
        .iter()
        .flat_map(|d| d.test_files.iter().map(|f| f.path.clone()))
        .collect()
}

/// Helper: collect all file entries (source + test).
pub fn all_file_entries(output: &TreeWalkerOutput) -> Vec<&FileEntry> {
    output
        .directories
        .iter()
        .flat_map(|d| d.source_files.iter().chain(d.test_files.iter()))
        .collect()
}

/// Resolve a relative path from tree walker output to an absolute path.
pub fn resolve_path(repo_path: &Path, relative: &str) -> PathBuf {
    repo_path.join(relative)
}

/// Generate a compact ls-R style tree diagram suitable for agent prompts.
/// Groups files with common prefixes and collapses when over budget.
pub fn generate_tree_diagram(tree: &TreeWalkerOutput, max_chars: usize) -> String {
    // Build a set of all directory paths and a map of dir -> file names.
    let mut dir_set: BTreeSet<String> = BTreeSet::new();
    let mut dir_files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut dir_file_counts: BTreeMap<String, usize> = BTreeMap::new();

    for dir_entry in &tree.directories {
        let dir_path = if dir_entry.path.is_empty() {
            ".".to_string()
        } else {
            dir_entry.path.clone()
        };

        // Register the directory and all ancestors.
        register_ancestors(&dir_path, &mut dir_set);

        let file_count = dir_entry.source_files.len() + dir_entry.test_files.len();
        dir_file_counts.insert(dir_path.clone(), file_count);

        let file_names: Vec<String> = dir_entry
            .source_files
            .iter()
            .chain(dir_entry.test_files.iter())
            .filter_map(|f| {
                f.path
                    .rsplit_once('/')
                    .map(|(_, name)| name.to_string())
                    .or_else(|| Some(f.path.clone()))
            })
            .collect();

        dir_files.insert(dir_path, file_names);
    }

    if dir_set.is_empty() && dir_files.is_empty() {
        return ".\n".to_string();
    }

    // Build parent -> sorted children map.
    let mut children_map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for dir in &dir_set {
        if dir == "." {
            continue;
        }
        let parent = match dir.rsplit_once('/') {
            Some((p, _)) => p.to_string(),
            None => ".".to_string(),
        };
        children_map
            .entry(parent)
            .or_default()
            .insert(dir.clone());
    }

    // Render with progressive collapse.
    let result = render_tree(".", &children_map, &dir_files, &dir_file_counts, max_chars);

    // If over budget, try collapsing deeper levels.
    if result.len() > max_chars {
        return render_tree_collapsed(
            ".",
            &children_map,
            &dir_files,
            &dir_file_counts,
            max_chars,
        );
    }

    result
}

/// Register a directory path and all its ancestors into the set.
fn register_ancestors(path: &str, dir_set: &mut BTreeSet<String>) {
    if path == "." || path.is_empty() {
        dir_set.insert(".".to_string());
        return;
    }
    dir_set.insert(path.to_string());
    let mut current = path;
    while let Some((parent, _)) = current.rsplit_once('/') {
        dir_set.insert(parent.to_string());
        current = parent;
    }
    dir_set.insert(".".to_string());
}

/// Compute the depth of a directory path (. = 0, src = 1, src/foo = 2).
fn dir_depth(path: &str) -> usize {
    if path == "." {
        0
    } else {
        path.matches('/').count() + 1
    }
}

/// Count total files in a subtree rooted at `dir`.
fn count_subtree_files(
    dir: &str,
    children_map: &BTreeMap<String, BTreeSet<String>>,
    dir_file_counts: &BTreeMap<String, usize>,
) -> usize {
    let own = dir_file_counts.get(dir).copied().unwrap_or(0);
    let child_sum: usize = children_map
        .get(dir)
        .map(|kids| {
            kids.iter()
                .map(|k| count_subtree_files(k, children_map, dir_file_counts))
                .sum()
        })
        .unwrap_or(0);
    own + child_sum
}

/// Group file names by common prefix (before `_` or `.`). Returns display lines.
fn group_files(files: &[String]) -> Vec<String> {
    if files.len() <= 5 {
        let mut sorted = files.to_vec();
        sorted.sort();
        return sorted;
    }

    // Find prefix groups: split each file on first `_` or `.` to get prefix.
    let mut prefix_counts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in files {
        let prefix = extract_prefix(f);
        prefix_counts.entry(prefix).or_default().push(f.clone());
    }

    let mut lines: Vec<String> = Vec::new();
    let mut ungrouped: Vec<String> = Vec::new();

    for (prefix, members) in &prefix_counts {
        if members.len() >= 3 {
            // Find common extension if any.
            let ext = common_extension(members);
            let ext_str = ext.map(|e| format!(".{e}")).unwrap_or_default();
            lines.push(format!("{prefix}_*{ext_str} ({} files)", members.len()));
        } else {
            ungrouped.extend(members.iter().cloned());
        }
    }

    ungrouped.sort();
    lines.extend(ungrouped);
    lines.sort();
    lines
}

/// Extract the grouping prefix from a filename (everything before the first `_` or `.`).
fn extract_prefix(name: &str) -> String {
    // Try underscore first for names like quality_foo.rs
    if let Some(idx) = name.find('_') {
        return name[..idx].to_string();
    }
    // Fall back to dot for names like foo.rs
    if let Some(idx) = name.find('.') {
        return name[..idx].to_string();
    }
    name.to_string()
}

/// Find common file extension among a group of files, if they all share one.
fn common_extension(files: &[String]) -> Option<String> {
    let mut ext: Option<String> = None;
    for f in files {
        let e = f.rsplit_once('.').map(|(_, e)| e.to_string());
        match (&ext, &e) {
            (None, Some(e)) => ext = Some(e.clone()),
            (Some(prev), Some(cur)) if prev == cur => {}
            _ => return None,
        }
    }
    ext
}

/// Render the tree with full expansion.
fn render_tree(
    root: &str,
    children_map: &BTreeMap<String, BTreeSet<String>>,
    dir_files: &BTreeMap<String, Vec<String>>,
    dir_file_counts: &BTreeMap<String, usize>,
    max_chars: usize,
) -> String {
    let mut out = String::from(".\n");
    render_node(
        root,
        "",
        true,
        children_map,
        dir_files,
        dir_file_counts,
        &mut out,
        max_chars,
        usize::MAX, // no depth limit
    );
    out
}

/// Render with progressive collapse: try depth 4, 3, then 2.
fn render_tree_collapsed(
    root: &str,
    children_map: &BTreeMap<String, BTreeSet<String>>,
    dir_files: &BTreeMap<String, Vec<String>>,
    dir_file_counts: &BTreeMap<String, usize>,
    max_chars: usize,
) -> String {
    for max_depth in (2..=4).rev() {
        let mut out = String::from(".\n");
        render_node(
            root,
            "",
            true,
            children_map,
            dir_files,
            dir_file_counts,
            &mut out,
            max_chars,
            max_depth,
        );
        if out.len() <= max_chars {
            return out;
        }
    }

    // Final fallback: depth 2, hard-truncate.
    let mut out = String::from(".\n");
    render_node(
        root,
        "",
        true,
        children_map,
        dir_files,
        dir_file_counts,
        &mut out,
        max_chars,
        2,
    );
    if out.len() > max_chars {
        out.truncate(max_chars.saturating_sub(4));
        out.push_str("...\n");
    }
    out
}

/// Recursive DFS render of a directory node.
#[allow(clippy::too_many_arguments)]
fn render_node(
    dir: &str,
    prefix: &str,
    _is_root: bool,
    children_map: &BTreeMap<String, BTreeSet<String>>,
    dir_files: &BTreeMap<String, Vec<String>>,
    dir_file_counts: &BTreeMap<String, usize>,
    out: &mut String,
    max_chars: usize,
    max_depth: usize,
) {
    let kids: Vec<String> = children_map
        .get(dir)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    let files = dir_files.get(dir).cloned().unwrap_or_default();

    // Collect all items to render: subdirs then files.
    let mut items: Vec<TreeItem> = Vec::new();
    for kid in &kids {
        let name = kid.rsplit_once('/').map(|(_, n)| n).unwrap_or(kid);
        let depth = dir_depth(kid);

        if depth >= max_depth {
            // Collapse this subtree.
            let count = count_subtree_files(kid, children_map, dir_file_counts);
            items.push(TreeItem::CollapsedDir(name.to_string(), count));
        } else {
            items.push(TreeItem::ExpandedDir(name.to_string(), kid.clone()));
        }
    }

    // Add files (grouped if needed).
    let file_lines = group_files(&files);
    for line in &file_lines {
        items.push(TreeItem::File(line.clone()));
    }

    let total = items.len();
    for (i, item) in items.iter().enumerate() {
        if out.len() >= max_chars {
            return;
        }
        let is_last = i + 1 == total;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = if is_last {
            format!("{prefix}    ")
        } else {
            format!("{prefix}│   ")
        };

        match item {
            TreeItem::ExpandedDir(name, full_path) => {
                out.push_str(&format!("{prefix}{connector}{name}/\n"));
                render_node(
                    full_path,
                    &child_prefix,
                    false,
                    children_map,
                    dir_files,
                    dir_file_counts,
                    out,
                    max_chars,
                    max_depth,
                );
            }
            TreeItem::CollapsedDir(name, count) => {
                if *count > 0 {
                    out.push_str(&format!("{prefix}{connector}{name}/ ({count} files)\n"));
                } else {
                    out.push_str(&format!("{prefix}{connector}{name}/\n"));
                }
            }
            TreeItem::File(line) => {
                out.push_str(&format!("{prefix}{connector}{line}\n"));
            }
        }
    }
}

enum TreeItem {
    ExpandedDir(String, String),
    CollapsedDir(String, usize),
    File(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn walk_repo_empty_dir() {
        let dir = tempdir().expect("tempdir");
        let output = walk_repo(dir.path());
        assert_eq!(output.total_source_files, 0);
        assert_eq!(output.total_test_files, 0);
        assert!(output.language_summary.is_empty());
    }

    #[test]
    fn walk_repo_finds_rust_source_file() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("create dir");
        fs::write(src.join("main.rs"), "fn main() {}\n").expect("write");
        let output = walk_repo(dir.path());
        assert_eq!(output.total_source_files, 1);
        assert_eq!(*output.language_summary.get("Rust").unwrap_or(&0), 1);
    }

    #[test]
    fn walk_repo_excludes_node_modules() {
        let dir = tempdir().expect("tempdir");
        let nm = dir.path().join("node_modules").join("pkg");
        fs::create_dir_all(&nm).expect("create dir");
        fs::write(nm.join("index.js"), "module.exports = {}").expect("write");
        let output = walk_repo(dir.path());
        assert_eq!(output.total_source_files, 0);
        assert!(output.excluded_directories.iter().any(|d| d.contains("node_modules")));
    }

    #[test]
    fn walk_repo_identifies_test_files() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("foo_test.go"), "package foo\n").expect("write");
        let output = walk_repo(dir.path());
        assert_eq!(output.total_test_files, 1);
    }

    fn make_tree(dirs: Vec<(&str, Vec<&str>)>) -> TreeWalkerOutput {
        let directories = dirs
            .into_iter()
            .map(|(path, files)| {
                let source_files = files
                    .iter()
                    .map(|f| {
                        let full = if path.is_empty() {
                            f.to_string()
                        } else {
                            format!("{path}/{f}")
                        };
                        FileEntry {
                            path: full,
                            language: "Rust".to_string(),
                            signature: Vec::new(),
                            line_count: 10,
                        }
                    })
                    .collect();
                DirectoryEntry {
                    path: path.to_string(),
                    source_files,
                    test_files: Vec::new(),
                }
            })
            .collect();
        TreeWalkerOutput {
            directories,
            language_summary: BTreeMap::new(),
            total_source_files: 0,
            total_test_files: 0,
            excluded_directories: Vec::new(),
        }
    }

    #[test]
    fn generate_tree_diagram_empty() {
        let tree = TreeWalkerOutput {
            directories: Vec::new(),
            language_summary: BTreeMap::new(),
            total_source_files: 0,
            total_test_files: 0,
            excluded_directories: Vec::new(),
        };
        let result = generate_tree_diagram(&tree, 10000);
        assert_eq!(result, ".\n");
    }

    #[test]
    fn generate_tree_diagram_simple_directory() {
        let tree = make_tree(vec![("src", vec!["main.rs", "lib.rs"])]);
        let result = generate_tree_diagram(&tree, 10000);
        assert!(result.starts_with(".\n"));
        assert!(result.contains("src/"));
        assert!(result.contains("main.rs"));
        assert!(result.contains("lib.rs"));
    }

    #[test]
    fn generate_tree_diagram_file_grouping() {
        let tree = make_tree(vec![(
            "src",
            vec![
                "quality_a.rs",
                "quality_b.rs",
                "quality_c.rs",
                "quality_d.rs",
                "quality_e.rs",
                "quality_f.rs",
                "other.rs",
            ],
        )]);
        let result = generate_tree_diagram(&tree, 10000);
        // Should group quality_* files since there are 6 of them (>= 3) and > 5 total files.
        assert!(
            result.contains("quality_*"),
            "Expected grouping, got:\n{result}"
        );
        assert!(result.contains("other.rs"));
    }

    #[test]
    fn generate_tree_diagram_max_chars_truncation() {
        // Build a tree with many directories to exceed budget.
        let dirs: Vec<(&str, Vec<&str>)> = (0..20)
            .map(|i| {
                // Leak strings so we get &str with 'static lifetime for the test.
                let dir: &str = Box::leak(format!("dir{i}").into_boxed_str());
                let file: &str = Box::leak(format!("file{i}.rs").into_boxed_str());
                (dir, vec![file])
            })
            .collect();
        let tree = make_tree(dirs);
        let result = generate_tree_diagram(&tree, 200);
        assert!(
            result.len() <= 204, // small tolerance for final "...\n"
            "Expected <= 204 chars, got {}:\n{result}",
            result.len()
        );
    }

    #[test]
    fn generate_tree_diagram_real_repo_under_budget() {
        // Walk the actual repo and verify the diagram stays compact.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("manifest parent")
            .parent()
            .expect("repo root");
        let tree = walk_repo(repo);
        let result = generate_tree_diagram(&tree, 5000);
        assert!(
            result.len() <= 5000,
            "Tree diagram is {} chars, expected <= 5000:\n{}",
            result.len(),
            &result[..result.len().min(500)]
        );
        assert!(result.starts_with(".\n"));
    }
}
