use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampledFile {
    pub path: String,
    pub content: String,
    pub line_count: usize,
}

/// Sample the top N files from a ranked list, reading their contents.
/// Stops when total_lines exceeds max_total_lines.
/// Returns files that fit within the budget.
pub fn sample_files(
    repo_path: &Path,
    ranked_paths: &[String],
    max_total_lines: usize,
) -> Vec<SampledFile> {
    let mut result = Vec::new();
    let mut total_lines = 0;

    for path in ranked_paths {
        let full_path = repo_path.join(path);
        let content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let line_count = content.lines().count();
        if total_lines + line_count > max_total_lines && !result.is_empty() {
            break;
        }

        total_lines += line_count;
        result.push(SampledFile {
            path: path.clone(),
            content,
            line_count,
        });

        if total_lines >= max_total_lines {
            break;
        }
    }

    result
}

/// Format sampled files into a string suitable for inclusion in a prompt.
pub fn format_sampled_files(files: &[SampledFile]) -> String {
    let mut out = String::new();
    for file in files {
        out.push_str(&format!(
            "### file: {}\n```\n{}\n```\n\n",
            file.path, file.content
        ));
    }
    out
}

/// Rank test files by assertion count (for test_quality hybrid agent).
/// Returns paths sorted by assertion count descending.
pub fn rank_test_files_by_assertions(
    assertion_files: &[(String, usize)], // (path, assertion_count)
) -> Vec<String> {
    let mut ranked: Vec<_> = assertion_files.to_vec();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ranked.into_iter().map(|(path, _)| path).collect()
}

/// Rank source files by complexity score (for risk_exposure hybrid agent).
/// Returns paths sorted by complexity descending.
pub fn rank_files_by_complexity(
    complexity_files: &[(String, f64)], // (path, complexity_score)
) -> Vec<String> {
    let mut ranked: Vec<_> = complexity_files.to_vec();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.into_iter().map(|(path, _)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn sample_files_empty_paths_returns_empty() {
        let dir = tempdir().expect("tempdir");
        let result = sample_files(dir.path(), &[], 1000);
        assert!(result.is_empty());
    }

    #[test]
    fn sample_files_respects_max_total_lines() {
        let dir = tempdir().expect("tempdir");
        // Create 3 files with 10 lines each
        for i in 0..3 {
            let content: String = (0..10).map(|j| format!("line {j}\n")).collect();
            fs::write(dir.path().join(format!("file{i}.rs")), &content).expect("write");
        }

        let paths: Vec<String> = (0..3).map(|i| format!("file{i}.rs")).collect();
        // Budget of 15 lines: should fit file0 (10) but stop before file1 would exceed
        let result = sample_files(dir.path(), &paths, 15);
        // First file (10 lines) fits. Second file (10 lines) would push to 20 > 15, so stop.
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].line_count, 10);
    }

    #[test]
    fn sample_files_always_includes_first_file_even_if_over_budget() {
        let dir = tempdir().expect("tempdir");
        let content: String = (0..50).map(|j| format!("line {j}\n")).collect();
        fs::write(dir.path().join("big.rs"), &content).expect("write");

        let paths = vec!["big.rs".to_string()];
        // Budget of 10 but should still include the first file
        let result = sample_files(dir.path(), &paths, 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].line_count, 50);
    }

    #[test]
    fn sample_files_reads_actual_file_contents() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("hello.rs"), "fn main() {}\n").expect("write");

        let paths = vec!["hello.rs".to_string()];
        let result = sample_files(dir.path(), &paths, 1000);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "hello.rs");
        assert!(result[0].content.contains("fn main()"));
        assert_eq!(result[0].line_count, 1);
    }

    #[test]
    fn sample_files_skips_missing_files() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("exists.rs"), "exists\n").expect("write");

        let paths = vec![
            "missing.rs".to_string(),
            "exists.rs".to_string(),
            "also_missing.rs".to_string(),
        ];
        let result = sample_files(dir.path(), &paths, 1000);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "exists.rs");
    }

    #[test]
    fn format_sampled_files_produces_expected_markdown() {
        let files = vec![
            SampledFile {
                path: "src/main.rs".to_string(),
                content: "fn main() {}".to_string(),
                line_count: 1,
            },
            SampledFile {
                path: "src/lib.rs".to_string(),
                content: "pub mod foo;".to_string(),
                line_count: 1,
            },
        ];

        let output = format_sampled_files(&files);
        assert!(output.contains("### file: src/main.rs"));
        assert!(output.contains("```\nfn main() {}\n```"));
        assert!(output.contains("### file: src/lib.rs"));
        assert!(output.contains("```\npub mod foo;\n```"));
    }

    #[test]
    fn rank_test_files_by_assertions_sorts_descending() {
        let files = vec![
            ("low.rs".to_string(), 2),
            ("high.rs".to_string(), 10),
            ("mid.rs".to_string(), 5),
        ];

        let ranked = rank_test_files_by_assertions(&files);
        assert_eq!(ranked, vec!["high.rs", "mid.rs", "low.rs"]);
    }

    #[test]
    fn rank_files_by_complexity_sorts_descending() {
        let files = vec![
            ("simple.rs".to_string(), 1.0),
            ("complex.rs".to_string(), 9.5),
            ("medium.rs".to_string(), 4.2),
        ];

        let ranked = rank_files_by_complexity(&files);
        assert_eq!(ranked, vec!["complex.rs", "medium.rs", "simple.rs"]);
    }
}
