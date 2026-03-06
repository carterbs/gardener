use crate::quality_tree_walker::TreeWalkerOutput;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntestedFinderOutput {
    pub files: Vec<UntestedFileEntry>,
    pub untested_count: usize,
    pub total_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntestedFileEntry {
    pub path: String,
    pub language: String,
    pub has_corresponding_test: bool,
    pub has_inline_tests: bool,
}

/// For each source file, determine whether a corresponding test exists.
///
/// A source file is considered "tested" if:
/// - A test file exists with a matching name pattern in a related directory
/// - The source file itself contains inline tests (checked via content)
pub fn find_untested(repo_path: &Path, tree: &TreeWalkerOutput) -> UntestedFinderOutput {
    // Build a map of (parent_dir, stem) → true for test files, so we match
    // tests to sources using both directory context and stem name.
    // Also include a global test stem set as a fallback (for tests/ directories
    // that are separate from the source tree).
    let mut local_test_stems: HashSet<(String, String)> = HashSet::new();
    let mut global_test_stems: HashSet<String> = HashSet::new();
    let mut test_paths: HashSet<String> = HashSet::new();

    for dir in &tree.directories {
        for tf in &dir.test_files {
            test_paths.insert(tf.path.clone());
            if let Some(stem) = extract_test_stem(&tf.path) {
                // Track test stem with its parent directory
                let parent = Path::new(&tf.path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                local_test_stems.insert((parent, stem.clone()));
                global_test_stems.insert(stem);
            }
        }
    }

    let mut files = Vec::new();
    let mut untested_count = 0;

    for dir in &tree.directories {
        for sf in &dir.source_files {
            // Skip if this file is also listed as a test file
            if test_paths.contains(&sf.path) {
                continue;
            }

            let source_stem = extract_source_stem(&sf.path).unwrap_or_default();
            let source_parent = Path::new(&sf.path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            // Check for corresponding test file: prefer same-directory match,
            // then check common test directories relative to the source dir,
            // then check sibling tests/ dirs (e.g., pkg/src/foo.rs -> pkg/tests/foo_test.rs),
            // then fall back to global stem match.
            let sibling_tests = {
                // For "pkg/src/foo.rs", check "pkg/tests"
                let p = Path::new(&source_parent);
                p.parent()
                    .map(|gp| {
                        let gp_str = gp.to_string_lossy().to_string();
                        if gp_str.is_empty() {
                            "tests".to_string()
                        } else {
                            format!("{gp_str}/tests")
                        }
                    })
                    .unwrap_or_default()
            };

            let has_corresponding_test = local_test_stems
                .contains(&(source_parent.clone(), source_stem.clone()))
                || local_test_stems.contains(&(format!("{source_parent}/tests"), source_stem.clone()))
                || local_test_stems.contains(&("tests".to_string(), source_stem.clone()))
                || local_test_stems
                    .contains(&(format!("tests/{}", dir.path), source_stem.clone()))
                || (!sibling_tests.is_empty()
                    && local_test_stems.contains(&(sibling_tests, source_stem.clone())))
                // Only use global match if there are no other source files with the same stem
                || (global_test_stems.contains(&source_stem)
                    && !has_duplicate_stem(tree, &sf.path, &source_stem));

            // Check for inline tests by reading file content
            let full_path = repo_path.join(&sf.path);
            let has_inline_tests = check_inline_tests(&full_path, &sf.language);

            let is_tested = has_corresponding_test || has_inline_tests;

            if !is_tested {
                untested_count += 1;
            }

            files.push(UntestedFileEntry {
                path: sf.path.clone(),
                language: sf.language.clone(),
                has_corresponding_test,
                has_inline_tests,
            });
        }
    }

    let total_count = files.len();

    UntestedFinderOutput {
        files,
        untested_count,
        total_count,
    }
}

/// Check if any other source file in the tree has the same stem (for dedup).
fn has_duplicate_stem(tree: &TreeWalkerOutput, my_path: &str, stem: &str) -> bool {
    tree.directories
        .iter()
        .flat_map(|d| d.source_files.iter())
        .any(|sf| sf.path != my_path && extract_source_stem(&sf.path).as_deref() == Some(stem))
}

/// Extract a normalized stem from a source file path for matching.
/// e.g. "src/backlog_store.rs" -> "backlog_store"
fn extract_source_stem(path: &str) -> Option<String> {
    let file_name = Path::new(path).file_stem()?.to_str()?;
    Some(file_name.to_ascii_lowercase())
}

/// Extract a normalized stem from a test file path, removing test prefixes/suffixes.
/// e.g. "tests/test_backlog.rs" -> "backlog"
/// e.g. "src/backlog_test.go" -> "backlog"
/// e.g. "src/backlog.test.ts" -> "backlog"
fn extract_test_stem(path: &str) -> Option<String> {
    let file_name = Path::new(path).file_stem()?.to_str()?;
    let lower = file_name.to_ascii_lowercase();

    // Remove common test patterns
    let stem = lower
        .strip_suffix("_test")
        .or_else(|| lower.strip_suffix(".test"))
        .or_else(|| lower.strip_suffix(".spec"))
        .or_else(|| lower.strip_suffix("_spec"))
        .or_else(|| lower.strip_prefix("test_"))
        .unwrap_or(&lower);

    // Handle double-extension like "foo.test" from "foo.test.ts"
    let stem = stem.strip_suffix(".test").unwrap_or(stem);
    let stem = stem.strip_suffix(".spec").unwrap_or(stem);

    Some(stem.to_string())
}

fn check_inline_tests(path: &Path, language: &str) -> bool {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    match language {
        "Rust" => {
            content.contains("#[cfg(test)]")
                || content.contains("#[test]")
                || content.contains("mod tests")
        }
        "Python" => content.contains("def test_") || content.contains("class Test"),
        "Go" => content.contains("func Test"),
        "TypeScript/JavaScript" => {
            content.contains("describe(") || content.contains("test(") || content.contains("it(")
        }
        "Swift" => content.contains("XCTestCase") || content.contains("func test"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality_tree_walker::{DirectoryEntry, FileEntry, TreeWalkerOutput};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    fn make_tree(source_files: Vec<FileEntry>, test_files: Vec<FileEntry>) -> TreeWalkerOutput {
        TreeWalkerOutput {
            directories: vec![DirectoryEntry {
                path: "src".to_string(),
                source_files,
                test_files,
            }],
            language_summary: BTreeMap::new(),
            total_source_files: 0,
            total_test_files: 0,
            excluded_directories: Vec::new(),
        }
    }

    #[test]
    fn find_untested_all_untested() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("create dir");
        fs::write(src.join("lib.rs"), "fn main() {}").expect("write");

        let tree = make_tree(
            vec![FileEntry {
                path: "src/lib.rs".to_string(),
                language: "Rust".to_string(),
                signature: Vec::new(),
                line_count: 1,
            }],
            Vec::new(),
        );

        let output = find_untested(dir.path(), &tree);
        assert_eq!(output.untested_count, 1);
        assert_eq!(output.total_count, 1);
    }

    #[test]
    fn find_untested_with_corresponding_test() {
        let dir = tempdir().expect("tempdir");
        let tree = make_tree(
            vec![FileEntry {
                path: "src/backlog.rs".to_string(),
                language: "Rust".to_string(),
                signature: Vec::new(),
                line_count: 10,
            }],
            vec![FileEntry {
                path: "tests/backlog_test.rs".to_string(),
                language: "Rust".to_string(),
                signature: Vec::new(),
                line_count: 5,
            }],
        );

        let output = find_untested(dir.path(), &tree);
        assert_eq!(output.untested_count, 0);
        assert!(output.files[0].has_corresponding_test);
    }

    #[test]
    fn find_untested_with_inline_tests() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("create dir");
        fs::write(
            src.join("lib.rs"),
            "#[cfg(test)]\nmod tests { #[test] fn works() {} }",
        )
        .expect("write");

        let tree = make_tree(
            vec![FileEntry {
                path: "src/lib.rs".to_string(),
                language: "Rust".to_string(),
                signature: Vec::new(),
                line_count: 2,
            }],
            Vec::new(),
        );

        let output = find_untested(dir.path(), &tree);
        assert_eq!(output.untested_count, 0);
        assert!(output.files[0].has_inline_tests);
    }

    #[test]
    fn extract_test_stem_strips_suffixes() {
        assert_eq!(
            extract_test_stem("tests/backlog_test.rs"),
            Some("backlog".to_string())
        );
        assert_eq!(
            extract_test_stem("test_backlog.py"),
            Some("backlog".to_string())
        );
        assert_eq!(extract_test_stem("foo.test.ts"), Some("foo".to_string()));
        assert_eq!(extract_test_stem("bar.spec.js"), Some("bar".to_string()));
    }
}
