use crate::quality_tree_walker::{all_file_entries, resolve_path, TreeWalkerOutput};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Debt marker keywords to scan for.
const DEBT_KEYWORDS: &[&str] = &["TODO", "FIXME", "HACK", "XXX", "DEPRECATED"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebtScannerOutput {
    pub markers: Vec<DebtMarker>,
    pub per_file_counts: BTreeMap<String, usize>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebtMarker {
    pub path: String,
    pub line_number: usize,
    pub keyword: String,
    pub context: String,
}

/// Scan all source and test files for debt markers (TODO, FIXME, HACK, XXX, DEPRECATED).
pub fn scan_debt(repo_path: &Path, tree: &TreeWalkerOutput) -> DebtScannerOutput {
    let mut markers = Vec::new();
    let mut per_file_counts: BTreeMap<String, usize> = BTreeMap::new();

    let all_files = all_file_entries(tree);

    for file_entry in all_files {
        let full_path = resolve_path(repo_path, &file_entry.path);
        let content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut file_count = 0;

        for (line_idx, line) in content.lines().enumerate() {
            // Convert to uppercase for case-insensitive matching of keywords
            let upper = line.to_ascii_uppercase();
            for keyword in DEBT_KEYWORDS {
                if upper.contains(keyword) {
                    // Verify it's likely a comment marker, not just part of a variable name.
                    // We check that the keyword appears after a comment indicator or at start.
                    if is_likely_comment_marker(&upper, keyword) {
                        markers.push(DebtMarker {
                            path: file_entry.path.clone(),
                            line_number: line_idx + 1,
                            keyword: keyword.to_string(),
                            context: line.trim().chars().take(200).collect(),
                        });
                        file_count += 1;
                        break; // Only count one marker per line
                    }
                }
            }
        }

        if file_count > 0 {
            per_file_counts.insert(file_entry.path.clone(), file_count);
        }
    }

    let total = markers.len();

    DebtScannerOutput {
        markers,
        per_file_counts,
        total,
    }
}

/// Check if a keyword occurrence is likely in a comment context.
fn is_likely_comment_marker(upper_line: &str, keyword: &str) -> bool {
    let trimmed = upper_line.trim();
    // Common comment prefixes across languages
    let comment_indicators = ["//", "#", "/*", "*", "--", "\"\"\"", "///"];

    // If the line starts with a comment indicator, it's likely a comment
    for indicator in &comment_indicators {
        let upper_indicator = indicator.to_ascii_uppercase();
        if trimmed.starts_with(&upper_indicator) {
            return true;
        }
    }

    // Check if keyword appears after a comment indicator anywhere in the line
    for indicator in &comment_indicators {
        if let Some(comment_pos) = upper_line.find(indicator) {
            if let Some(keyword_pos) = upper_line.find(keyword) {
                if keyword_pos > comment_pos {
                    return true;
                }
            }
        }
    }

    // Also match if keyword is followed by colon or parenthesis (common pattern: TODO: fix this)
    if let Some(pos) = upper_line.find(keyword) {
        let after = &upper_line[pos + keyword.len()..];
        if after.starts_with(':')
            || after.starts_with('(')
            || after.starts_with(' ')
            || after.is_empty()
        {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality_tree_walker::{DirectoryEntry, FileEntry, TreeWalkerOutput};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    fn make_tree(files: Vec<(&str, &str)>) -> (tempfile::TempDir, TreeWalkerOutput) {
        let dir = tempdir().expect("tempdir");
        let mut source_files = Vec::new();

        for (name, content) in &files {
            fs::write(dir.path().join(name), content).expect("write");
            source_files.push(FileEntry {
                path: name.to_string(),
                language: "Rust".to_string(),
                signature: Vec::new(),
                line_count: content.lines().count(),
            });
        }

        let tree = TreeWalkerOutput {
            directories: vec![DirectoryEntry {
                path: ".".to_string(),
                source_files,
                test_files: Vec::new(),
            }],
            language_summary: BTreeMap::new(),
            total_source_files: files.len(),
            total_test_files: 0,
            excluded_directories: Vec::new(),
        };

        (dir, tree)
    }

    #[test]
    fn scan_debt_finds_todo() {
        let (dir, tree) = make_tree(vec![("main.rs", "// TODO: fix this\nfn main() {}")]);
        let output = scan_debt(dir.path(), &tree);
        assert_eq!(output.total, 1);
        assert_eq!(output.markers[0].keyword, "TODO");
    }

    #[test]
    fn scan_debt_finds_fixme() {
        let (dir, tree) = make_tree(vec![("lib.rs", "// FIXME: broken\nfn f() {}")]);
        let output = scan_debt(dir.path(), &tree);
        assert_eq!(output.total, 1);
        assert_eq!(output.markers[0].keyword, "FIXME");
    }

    #[test]
    fn scan_debt_empty_file() {
        let (dir, tree) = make_tree(vec![("empty.rs", "fn main() {}")]);
        let output = scan_debt(dir.path(), &tree);
        assert_eq!(output.total, 0);
    }

    #[test]
    fn scan_debt_multiple_markers() {
        let content = "// TODO: first\n// HACK: second\n// FIXME: third\n";
        let (dir, tree) = make_tree(vec![("multi.rs", content)]);
        let output = scan_debt(dir.path(), &tree);
        assert_eq!(output.total, 3);
    }
}
