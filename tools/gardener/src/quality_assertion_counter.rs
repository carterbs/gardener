use crate::quality_language_registry::definition_for_language;
use crate::quality_test_detector::TestDetectorOutput;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionCounterOutput {
    pub files: Vec<AssertionFileEntry>,
    pub totals: AssertionTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionFileEntry {
    pub path: String,
    pub language: String,
    pub assertion_count: usize,
    pub line_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionTotals {
    pub total_assertions: usize,
    pub total_test_files: usize,
    pub avg_assertions_per_file: f64,
}

/// Count assertions in all detected test files using language-specific patterns.
pub fn count_assertions(repo_path: &Path, tests: &TestDetectorOutput) -> AssertionCounterOutput {
    let mut files: Vec<AssertionFileEntry> = Vec::new();
    let mut total_assertions = 0usize;

    for test_file in &tests.test_files {
        let full_path = repo_path.join(&test_file.path);
        let content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let line_count = content.lines().count();
        let lang_def = definition_for_language(&test_file.language);
        let patterns: &[String] = lang_def
            .as_ref()
            .map(|ld| ld.assertion_patterns.as_slice())
            .unwrap_or(&[]);

        let assertion_count = count_pattern_occurrences(&content, patterns);
        total_assertions += assertion_count;

        files.push(AssertionFileEntry {
            path: test_file.path.clone(),
            language: test_file.language.clone(),
            assertion_count,
            line_count,
        });
    }

    let total_test_files = files.len();
    let avg_assertions_per_file = if total_test_files > 0 {
        total_assertions as f64 / total_test_files as f64
    } else {
        0.0
    };

    AssertionCounterOutput {
        files,
        totals: AssertionTotals {
            total_assertions,
            total_test_files,
            avg_assertions_per_file,
        },
    }
}

fn count_pattern_occurrences(content: &str, patterns: &[String]) -> usize {
    let mut count = 0;
    for line in content.lines() {
        for pattern in patterns {
            // Count non-overlapping occurrences per line
            count += line.matches(pattern.as_str()).count();
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality_test_detector::{TestDetectorOutput, TestFileEntry};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn count_assertions_empty() {
        let dir = tempdir().expect("tempdir");
        let tests = TestDetectorOutput {
            test_files: Vec::new(),
            untested_source_files: Vec::new(),
            summary: BTreeMap::new(),
        };
        let output = count_assertions(dir.path(), &tests);
        assert_eq!(output.totals.total_assertions, 0);
        assert_eq!(output.totals.total_test_files, 0);
    }

    #[test]
    fn count_assertions_in_rust_file() {
        let dir = tempdir().expect("tempdir");
        let test_content = r#"
#[test]
fn test_add() {
    assert_eq!(2 + 2, 4);
    assert!(true);
    assert_ne!(1, 2);
}
"#;
        fs::write(dir.path().join("test_math.rs"), test_content).expect("write");
        let tests = TestDetectorOutput {
            test_files: vec![TestFileEntry {
                path: "test_math.rs".to_string(),
                language: "Rust".to_string(),
                test_type: "unit".to_string(),
                detected_by: "path_pattern".to_string(),
            }],
            untested_source_files: Vec::new(),
            summary: BTreeMap::new(),
        };
        let output = count_assertions(dir.path(), &tests);
        assert!(output.totals.total_assertions >= 3);
    }
}
