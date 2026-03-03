use crate::quality_language_registry::identify_language;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

    for line in reader.lines().flatten() {
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
}
