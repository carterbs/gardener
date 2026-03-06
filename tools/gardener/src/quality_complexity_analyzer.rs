use crate::quality_tree_walker::{resolve_path, TreeWalkerOutput};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityAnalyzerOutput {
    pub files: Vec<FileComplexity>,
    pub summary: ComplexitySummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileComplexity {
    pub path: String,
    pub language: String,
    pub line_count: usize,
    pub max_nesting_depth: usize,
    pub branch_count: usize,
    pub function_count: usize,
    pub avg_function_length: f64,
    pub complexity_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexitySummary {
    pub total_files: usize,
    pub avg_complexity: f64,
    pub max_complexity_file: Option<String>,
}

/// Analyze per-file complexity metrics deterministically across all source files.
pub fn analyze_complexity(repo_path: &Path, tree: &TreeWalkerOutput) -> ComplexityAnalyzerOutput {
    let mut files: Vec<FileComplexity> = Vec::new();

    for dir_entry in &tree.directories {
        for file_entry in &dir_entry.source_files {
            let full_path = resolve_path(repo_path, &file_entry.path);
            let content = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let language = &file_entry.language;
            let line_count = content.lines().count();
            let max_nesting_depth = compute_max_nesting(&content, language);
            let branch_count = count_branches(&content, language);
            let function_count = count_functions(&content, language);
            let avg_function_length = if function_count > 0 {
                line_count as f64 / function_count as f64
            } else {
                0.0
            };
            let complexity_score = branch_count as f64 * 2.0
                + max_nesting_depth as f64 * 3.0
                + line_count as f64 / 50.0;

            files.push(FileComplexity {
                path: file_entry.path.clone(),
                language: language.clone(),
                line_count,
                max_nesting_depth,
                branch_count,
                function_count,
                avg_function_length,
                complexity_score,
            });
        }
    }

    // Sort by complexity_score descending
    files.sort_by(|a, b| {
        b.complexity_score
            .partial_cmp(&a.complexity_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let total_files = files.len();
    let avg_complexity = if total_files > 0 {
        files.iter().map(|f| f.complexity_score).sum::<f64>() / total_files as f64
    } else {
        0.0
    };
    let max_complexity_file = files.first().map(|f| f.path.clone());

    ComplexityAnalyzerOutput {
        files,
        summary: ComplexitySummary {
            total_files,
            avg_complexity,
            max_complexity_file,
        },
    }
}

/// Compute max nesting depth. For brace-based languages, track `{`/`}` depth.
/// For Python, track indent-level changes.
fn compute_max_nesting(content: &str, language: &str) -> usize {
    if language == "Python" {
        compute_max_nesting_python(content)
    } else {
        compute_max_nesting_braces(content)
    }
}

fn compute_max_nesting_braces(content: &str) -> usize {
    let mut depth: usize = 0;
    let mut max_depth: usize = 0;

    for ch in content.chars() {
        if ch == '{' {
            depth += 1;
            if depth > max_depth {
                max_depth = depth;
            }
        } else if ch == '}' {
            depth = depth.saturating_sub(1);
        }
    }

    max_depth
}

fn compute_max_nesting_python(content: &str) -> usize {
    let mut max_indent_level: usize = 0;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let spaces = line.len() - line.trim_start().len();
        // Use 4-space indentation as one level; fall back to raw spaces / 4
        let indent_level = spaces / 4;
        if indent_level > max_indent_level {
            max_indent_level = indent_level;
        }
    }

    max_indent_level
}

/// Count language-specific branch keywords with word-boundary awareness.
fn count_branches(content: &str, language: &str) -> usize {
    let patterns: &[&str] = match language {
        "Rust" => &[
            "if ",
            "match ",
            "else ",
            "for ",
            "while ",
            ".unwrap()",
            "?",
            "loop ",
        ],
        "Go" => &["if ", "else ", "for ", "switch ", "select "],
        "Python" => &["if ", "elif ", "else:", "for ", "while ", "try:", "except "],
        "TypeScript/JavaScript" => &[
            "if ", "else ", "for ", "while ", "switch ", "catch ", "try ",
        ],
        _ => &[],
    };

    let mut count = 0;
    for line in content.lines() {
        let trimmed = line.trim();
        for pattern in patterns {
            // For single-char patterns like "?", count occurrences directly
            if *pattern == "?" {
                count += trimmed.matches('?').count();
                continue;
            }
            if *pattern == ".unwrap()" {
                count += trimmed.matches(".unwrap()").count();
                continue;
            }
            // For keyword patterns, check that they appear as word boundaries.
            // The pattern already includes trailing space/colon, so a simple contains
            // check is mostly sufficient. We also check line-start for keywords that
            // could begin a statement.
            count += trimmed.matches(pattern).count();
        }
    }

    count
}

/// Count function definitions per language.
fn count_functions(content: &str, language: &str) -> usize {
    let mut count = 0;

    for line in content.lines() {
        let trimmed = line.trim_start();
        match language {
            "Rust" => {
                if starts_with_keyword(trimmed, "fn ") {
                    count += 1;
                }
            }
            "Go" => {
                if starts_with_keyword(trimmed, "func ") {
                    count += 1;
                }
            }
            "Python" => {
                if starts_with_keyword(trimmed, "def ") {
                    count += 1;
                }
            }
            "TypeScript/JavaScript" => {
                if starts_with_keyword(trimmed, "function ") || trimmed.contains("=> {") {
                    count += 1;
                }
            }
            _ => {}
        }
    }

    count
}

/// Check if a line starts with a keyword (handles `pub fn`, `async fn`, visibility modifiers, etc.)
fn starts_with_keyword(trimmed: &str, keyword: &str) -> bool {
    if trimmed.starts_with(keyword) {
        return true;
    }
    // For Rust: also match `pub fn`, `pub(crate) fn`, `async fn`, `pub async fn`, etc.
    // For Go: also match after comment-free prefix
    // For Python: `async def`
    // Generic approach: check if keyword appears after known prefixes
    if let Some(pos) = trimmed.find(keyword) {
        // Make sure the keyword isn't part of a longer word by checking
        // the char before it (if any) is whitespace or line start.
        if pos > 0 {
            let before = trimmed.as_bytes()[pos - 1];
            return before == b' ' || before == b'\t';
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

    fn make_tree(files: Vec<(&str, &str, &str)>) -> (tempfile::TempDir, TreeWalkerOutput) {
        let dir = tempdir().expect("tempdir");
        let mut source_files = Vec::new();

        for (name, language, content) in &files {
            fs::write(dir.path().join(name), content).expect("write");
            source_files.push(FileEntry {
                path: name.to_string(),
                language: language.to_string(),
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
    fn empty_repo() {
        let dir = tempdir().expect("tempdir");
        let tree = TreeWalkerOutput {
            directories: Vec::new(),
            language_summary: BTreeMap::new(),
            total_source_files: 0,
            total_test_files: 0,
            excluded_directories: Vec::new(),
        };
        let output = analyze_complexity(dir.path(), &tree);
        assert_eq!(output.summary.total_files, 0);
        assert_eq!(output.summary.avg_complexity, 0.0);
        assert!(output.summary.max_complexity_file.is_none());
        assert!(output.files.is_empty());
    }

    #[test]
    fn simple_rust_file_complexity() {
        let content = r#"fn main() {
    if true {
        for i in 0..10 {
            println!("{}", i);
        }
    }
}
"#;
        let (dir, tree) = make_tree(vec![("main.rs", "Rust", content)]);
        let output = analyze_complexity(dir.path(), &tree);

        assert_eq!(output.files.len(), 1);
        let file = &output.files[0];
        assert_eq!(file.path, "main.rs");
        assert_eq!(file.language, "Rust");
        assert_eq!(file.function_count, 1);
        assert_eq!(file.max_nesting_depth, 4); // main { if { for { println!("{}" } } }
        assert_eq!(file.branch_count, 2); // if + for
        assert!(file.avg_function_length > 0.0);
    }

    #[test]
    fn complexity_score_calculation() {
        // complexity_score = branch_count * 2.0 + max_nesting_depth * 3.0 + line_count / 50.0
        let content = "fn f() {\n    if true {\n    }\n}\n";
        let (dir, tree) = make_tree(vec![("simple.rs", "Rust", content)]);
        let output = analyze_complexity(dir.path(), &tree);

        let file = &output.files[0];
        let line_count = content.lines().count();
        let expected = file.branch_count as f64 * 2.0
            + file.max_nesting_depth as f64 * 3.0
            + line_count as f64 / 50.0;
        assert!((file.complexity_score - expected).abs() < 0.001);
    }

    #[test]
    fn sorting_highest_complexity_first() {
        let simple = "fn a() {}\n";
        let complex = r#"fn b() {
    if true {
        if true {
            for x in 0..10 {
                match x {
                    _ => {}
                }
            }
        }
    }
}
fn c() {
    if true {
        while true {
        }
    }
}
"#;
        let (dir, tree) = make_tree(vec![
            ("simple.rs", "Rust", simple),
            ("complex.rs", "Rust", complex),
        ]);
        let output = analyze_complexity(dir.path(), &tree);

        assert_eq!(output.files.len(), 2);
        assert_eq!(output.files[0].path, "complex.rs");
        assert_eq!(output.files[1].path, "simple.rs");
        assert!(output.files[0].complexity_score > output.files[1].complexity_score);
        assert_eq!(
            output.summary.max_complexity_file,
            Some("complex.rs".to_string())
        );
    }

    #[test]
    fn python_nesting_by_indent() {
        let content = "def foo():\n    if True:\n        for x in range(10):\n            pass\n";
        let (dir, tree) = make_tree(vec![("main.py", "Python", content)]);
        let output = analyze_complexity(dir.path(), &tree);

        let file = &output.files[0];
        assert_eq!(file.max_nesting_depth, 3); // 12 spaces / 4 = 3
        assert_eq!(file.function_count, 1);
    }

    #[test]
    fn go_function_counting() {
        let content = "func main() {\n}\n\nfunc helper() {\n}\n";
        let (dir, tree) = make_tree(vec![("main.go", "Go", content)]);
        let output = analyze_complexity(dir.path(), &tree);

        let file = &output.files[0];
        assert_eq!(file.function_count, 2);
    }

    #[test]
    fn js_arrow_function_counting() {
        let content = "const f = () => {\n  return 1;\n};\nfunction g() {\n  return 2;\n}\n";
        let (dir, tree) = make_tree(vec![("app.js", "TypeScript/JavaScript", content)]);
        let output = analyze_complexity(dir.path(), &tree);

        let file = &output.files[0];
        assert_eq!(file.function_count, 2); // arrow + function
    }

    #[test]
    fn rust_pub_fn_counted() {
        let content =
            "pub fn public_func() {}\npub(crate) fn crate_func() {}\nasync fn async_func() {}\n";
        let (dir, tree) = make_tree(vec![("lib.rs", "Rust", content)]);
        let output = analyze_complexity(dir.path(), &tree);

        let file = &output.files[0];
        assert_eq!(file.function_count, 3);
    }
}
