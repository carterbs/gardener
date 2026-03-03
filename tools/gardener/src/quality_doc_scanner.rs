use serde::{Deserialize, Serialize};
use std::path::Path;

/// Max lines before truncating doc content in the output.
const MAX_INLINE_LINES: usize = 500;

/// Steering docs: files that guide agent/human behavior.
const STEERING_DOC_PATHS: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "CODEX.md",
    ".github/AGENTS.md",
    ".github/CLAUDE.md",
    ".cursorrules",
    ".windsurfrules",
];

/// Convention docs: files that describe project conventions.
const CONVENTION_DOC_PATHS: &[&str] = &[
    "README.md",
    "README",
    "README.txt",
    "CONTRIBUTING.md",
    ".editorconfig",
    "STYLE.md",
    "ARCHITECTURE.md",
    "CODE_OF_CONDUCT.md",
];

/// Directory patterns for additional docs.
const DOC_DIRECTORY_PATTERNS: &[&str] = &[
    "docs",
    "documentation",
    "doc",
    "docs/conventions",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocScannerOutput {
    pub docs: Vec<DocEntry>,
    pub steering_doc_count: usize,
    pub convention_doc_count: usize,
    pub total_doc_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEntry {
    pub path: String,
    pub doc_type: String,
    pub line_count: usize,
    pub content: Option<String>,
    pub truncated: bool,
}

/// Scan for steering and convention documentation files in the repo.
pub fn scan_docs(repo_path: &Path) -> DocScannerOutput {
    let mut docs = Vec::new();
    let mut steering_count = 0usize;
    let mut convention_count = 0usize;

    // Check steering docs
    for path in STEERING_DOC_PATHS {
        let full_path = repo_path.join(path);
        if full_path.is_file() {
            if let Some(entry) = read_doc_entry(&full_path, path, "steering") {
                docs.push(entry);
                steering_count += 1;
            }
        }
    }

    // Check convention docs
    for path in CONVENTION_DOC_PATHS {
        let full_path = repo_path.join(path);
        if full_path.is_file() {
            if let Some(entry) = read_doc_entry(&full_path, path, "convention") {
                docs.push(entry);
                convention_count += 1;
            }
        }
    }

    // Scan doc directories for markdown files
    for dir_pattern in DOC_DIRECTORY_PATTERNS {
        let dir_path = repo_path.join(dir_pattern);
        if dir_path.is_dir() {
            scan_doc_directory(&dir_path, repo_path, &mut docs, &mut convention_count);
        }
    }

    let total_doc_files = docs.len();

    DocScannerOutput {
        docs,
        steering_doc_count: steering_count,
        convention_doc_count: convention_count,
        total_doc_files,
    }
}

fn read_doc_entry(full_path: &Path, relative_path: &str, doc_type: &str) -> Option<DocEntry> {
    let content_str = std::fs::read_to_string(full_path).ok()?;
    let line_count = content_str.lines().count();
    let truncated = line_count > MAX_INLINE_LINES;

    let content = if line_count <= MAX_INLINE_LINES {
        Some(content_str)
    } else {
        // Include first MAX_INLINE_LINES lines
        let truncated_content: String = content_str
            .lines()
            .take(MAX_INLINE_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        Some(truncated_content)
    };

    Some(DocEntry {
        path: relative_path.to_string(),
        doc_type: doc_type.to_string(),
        line_count,
        content,
        truncated,
    })
}

fn scan_doc_directory(
    dir: &Path,
    repo_root: &Path,
    docs: &mut Vec<DocEntry>,
    convention_count: &mut usize,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_doc_directory(&path, repo_root, docs, convention_count);
            continue;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if ext != "md" && ext != "txt" && ext != "rst" {
            continue;
        }

        let relative = path
            .strip_prefix(repo_root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_string();

        // Skip if we already have this doc (from steering/convention checks)
        if docs.iter().any(|d| d.path == relative) {
            continue;
        }

        if let Some(entry) = read_doc_entry(&path, &relative, "convention") {
            docs.push(entry);
            *convention_count += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn scan_docs_empty_repo() {
        let dir = tempdir().expect("tempdir");
        let output = scan_docs(dir.path());
        assert_eq!(output.total_doc_files, 0);
        assert_eq!(output.steering_doc_count, 0);
        assert_eq!(output.convention_doc_count, 0);
    }

    #[test]
    fn scan_docs_finds_steering_doc() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("AGENTS.md"), "# Agent instructions\n").expect("write");
        let output = scan_docs(dir.path());
        assert_eq!(output.steering_doc_count, 1);
        assert_eq!(output.docs[0].doc_type, "steering");
    }

    #[test]
    fn scan_docs_finds_readme() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("README.md"), "# Project\n").expect("write");
        let output = scan_docs(dir.path());
        assert_eq!(output.convention_doc_count, 1);
    }

    #[test]
    fn scan_docs_truncates_large_files() {
        let dir = tempdir().expect("tempdir");
        let large_content: String = (0..600).map(|i| format!("line {i}\n")).collect();
        fs::write(dir.path().join("AGENTS.md"), &large_content).expect("write");
        let output = scan_docs(dir.path());
        assert!(output.docs[0].truncated);
    }

    #[test]
    fn scan_docs_includes_docs_directory() {
        let dir = tempdir().expect("tempdir");
        let docs_dir = dir.path().join("docs");
        fs::create_dir_all(&docs_dir).expect("create dir");
        fs::write(docs_dir.join("architecture.md"), "# Arch\n").expect("write");
        let output = scan_docs(dir.path());
        assert!(output.docs.iter().any(|d| d.path.contains("architecture.md")));
    }
}
