use crate::quality_language_registry::{
    classify_test_type, definition_for_language, TestFileIndicator, TestType,
};
use crate::quality_tree_walker::{resolve_path, TreeWalkerOutput};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDetectorOutput {
    pub test_files: Vec<TestFileEntry>,
    pub untested_source_files: Vec<String>,
    pub summary: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFileEntry {
    pub path: String,
    pub language: String,
    pub test_type: String,
    pub detected_by: String,
}

/// Detect test files from tree walker output, classifying each by type.
pub fn detect_tests(repo_path: &Path, tree: &TreeWalkerOutput) -> TestDetectorOutput {
    let mut test_files: Vec<TestFileEntry> = Vec::new();
    let mut test_file_set: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Collect files already marked as test files by tree walker
    for dir in &tree.directories {
        for tf in &dir.test_files {
            let lang_def = definition_for_language(&tf.language);
            let test_type = lang_def
                .as_ref()
                .map(|ld| classify_test_type(Path::new(&tf.path), ld))
                .unwrap_or(TestType::Unknown);

            test_files.push(TestFileEntry {
                path: tf.path.clone(),
                language: tf.language.clone(),
                test_type: test_type.as_str().to_string(),
                detected_by: "path_pattern".to_string(),
            });
            test_file_set.insert(tf.path.clone());
        }
    }

    // Also check source files for inline test content (e.g. Rust #[cfg(test)])
    for dir in &tree.directories {
        for sf in &dir.source_files {
            if test_file_set.contains(&sf.path) {
                continue;
            }

            let lang_def = match definition_for_language(&sf.language) {
                Some(ld) => ld,
                None => continue,
            };

            let full_path = resolve_path(repo_path, &sf.path);
            if let Some(detected_by) = check_content_indicators(&full_path, &lang_def.test_file_indicators) {
                test_files.push(TestFileEntry {
                    path: sf.path.clone(),
                    language: sf.language.clone(),
                    test_type: TestType::Unit.as_str().to_string(),
                    detected_by,
                });
                test_file_set.insert(sf.path.clone());
            }
        }
    }

    // Determine untested source files: source files without a corresponding test
    let untested_source_files: Vec<String> = tree
        .directories
        .iter()
        .flat_map(|d| d.source_files.iter())
        .filter(|sf| !test_file_set.contains(&sf.path))
        .map(|sf| sf.path.clone())
        .collect();

    // Build summary by test type
    let mut summary: BTreeMap<String, usize> = BTreeMap::new();
    for tf in &test_files {
        *summary.entry(tf.test_type.clone()).or_insert(0) += 1;
    }

    TestDetectorOutput {
        test_files,
        untested_source_files,
        summary,
    }
}

/// Check if a file's content matches any ContentMatch indicators.
fn check_content_indicators(path: &Path, indicators: &[TestFileIndicator]) -> Option<String> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return None,
    };

    for indicator in indicators {
        if let TestFileIndicator::ContentMatch(pattern) = indicator {
            if content.contains(pattern.as_str()) {
                return Some(format!("content_match:{pattern}"));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality_tree_walker::{DirectoryEntry, FileEntry, TreeWalkerOutput};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    fn empty_tree() -> TreeWalkerOutput {
        TreeWalkerOutput {
            directories: Vec::new(),
            language_summary: BTreeMap::new(),
            total_source_files: 0,
            total_test_files: 0,
            excluded_directories: Vec::new(),
        }
    }

    #[test]
    fn detect_tests_empty_tree() {
        let dir = tempdir().expect("tempdir");
        let output = detect_tests(dir.path(), &empty_tree());
        assert!(output.test_files.is_empty());
        assert!(output.untested_source_files.is_empty());
    }

    #[test]
    fn detect_tests_finds_test_files_from_tree() {
        let dir = tempdir().expect("tempdir");
        let tree = TreeWalkerOutput {
            directories: vec![DirectoryEntry {
                path: "src".to_string(),
                source_files: vec![FileEntry {
                    path: "src/lib.rs".to_string(),
                    language: "Rust".to_string(),
                    signature: Vec::new(),
                    line_count: 10,
                }],
                test_files: vec![FileEntry {
                    path: "tests/integration.rs".to_string(),
                    language: "Rust".to_string(),
                    signature: Vec::new(),
                    line_count: 20,
                }],
            }],
            language_summary: BTreeMap::new(),
            total_source_files: 1,
            total_test_files: 1,
            excluded_directories: Vec::new(),
        };

        let output = detect_tests(dir.path(), &tree);
        assert_eq!(output.test_files.len(), 1);
        assert_eq!(output.test_files[0].test_type, "integration");
    }

    #[test]
    fn detect_tests_finds_inline_rust_tests() {
        let dir = tempdir().expect("tempdir");
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).expect("create dir");
        fs::write(
            src_dir.join("lib.rs"),
            "fn main() {}\n\n#[cfg(test)]\nmod tests { #[test] fn it_works() {} }",
        )
        .expect("write");

        let tree = TreeWalkerOutput {
            directories: vec![DirectoryEntry {
                path: "src".to_string(),
                source_files: vec![FileEntry {
                    path: "src/lib.rs".to_string(),
                    language: "Rust".to_string(),
                    signature: Vec::new(),
                    line_count: 4,
                }],
                test_files: Vec::new(),
            }],
            language_summary: BTreeMap::new(),
            total_source_files: 1,
            total_test_files: 0,
            excluded_directories: Vec::new(),
        };

        let output = detect_tests(dir.path(), &tree);
        assert_eq!(output.test_files.len(), 1);
        assert!(output.test_files[0].detected_by.starts_with("content_match"));
    }
}
