use crate::quality_language_registry::definition_for_language;
use crate::quality_tree_walker::{resolve_path, TreeWalkerOutput};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentationDetectorOutput {
    pub files_with_instrumentation: usize,
    pub total_source_files: usize,
    pub instrumentation_ratio: f64,
    pub frameworks_detected: Vec<String>,
    pub per_file: Vec<InstrumentationFileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentationFileEntry {
    pub path: String,
    pub language: String,
    pub has_instrumentation: bool,
    pub patterns_found: Vec<String>,
}

/// Scan source files for logging, tracing, and metrics instrumentation.
pub fn detect_instrumentation(
    repo_path: &Path,
    tree: &TreeWalkerOutput,
) -> InstrumentationDetectorOutput {
    let mut per_file = Vec::new();
    let mut files_with = 0usize;
    let mut total = 0usize;
    let mut frameworks: BTreeSet<String> = BTreeSet::new();

    let all_source = tree.directories.iter().flat_map(|d| d.source_files.iter());

    for file_entry in all_source {
        total += 1;
        let full_path = resolve_path(repo_path, &file_entry.path);
        let content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(_) => {
                per_file.push(InstrumentationFileEntry {
                    path: file_entry.path.clone(),
                    language: file_entry.language.clone(),
                    has_instrumentation: false,
                    patterns_found: Vec::new(),
                });
                continue;
            }
        };

        let lang_def = definition_for_language(&file_entry.language);
        let patterns: &[String] = lang_def
            .as_ref()
            .map(|ld| ld.instrumentation_patterns.as_slice())
            .unwrap_or(&[]);

        let mut found_patterns: Vec<String> = Vec::new();
        for pattern in patterns {
            if content.contains(pattern.as_str()) {
                found_patterns.push(pattern.clone());
                // Map patterns to framework names
                let framework = pattern_to_framework(pattern);
                if !framework.is_empty() {
                    frameworks.insert(framework.to_string());
                }
            }
        }

        let has_instrumentation = !found_patterns.is_empty();
        if has_instrumentation {
            files_with += 1;
        }

        per_file.push(InstrumentationFileEntry {
            path: file_entry.path.clone(),
            language: file_entry.language.clone(),
            has_instrumentation,
            patterns_found: found_patterns,
        });
    }

    let instrumentation_ratio = if total > 0 {
        files_with as f64 / total as f64
    } else {
        0.0
    };

    InstrumentationDetectorOutput {
        files_with_instrumentation: files_with,
        total_source_files: total,
        instrumentation_ratio,
        frameworks_detected: frameworks.into_iter().collect(),
        per_file,
    }
}

fn pattern_to_framework(pattern: &str) -> &str {
    match pattern {
        p if p.contains("tracing::") || p.contains("tracing_subscriber") => "tracing (Rust)",
        p if p.contains("log::") && !p.contains("console.log") => "log (Rust)",
        p if p.contains("env_logger") => "env_logger (Rust)",
        p if p.contains("append_run_log") => "gardener logging",
        p if p.contains("console.log")
            || p.contains("console.error")
            || p.contains("console.warn") =>
        {
            "console (JS)"
        }
        p if p.contains("winston") => "winston (JS)",
        p if p.contains("pino") => "pino (JS)",
        p if p.contains("bunyan") => "bunyan (JS)",
        p if p.contains("log4js") => "log4js (JS)",
        p if p.contains("os_log") || p.contains("OSLog") => "os_log (Swift)",
        p if p.contains("Logger(") && !p.contains("getLogger") => "Logger (Swift)",
        p if p.contains("NSLog") => "NSLog (Swift)",
        p if p.contains("logging.") || p.contains("logger.") || p.contains("getLogger") => {
            "logging (Python)"
        }
        p if p.contains("structlog") => "structlog (Python)",
        p if p.contains("loguru") => "loguru (Python)",
        p if p.contains("zap.") => "zap (Go)",
        p if p.contains("logrus.") => "logrus (Go)",
        p if p.contains("zerolog") => "zerolog (Go)",
        p if p.contains("slog.") => "slog (Go)",
        p if p.contains("klog.") => "klog (Go)",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality_tree_walker::{DirectoryEntry, FileEntry, TreeWalkerOutput};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detect_instrumentation_empty() {
        let dir = tempdir().expect("tempdir");
        let tree = TreeWalkerOutput {
            directories: Vec::new(),
            language_summary: BTreeMap::new(),
            total_source_files: 0,
            total_test_files: 0,
            excluded_directories: Vec::new(),
        };
        let output = detect_instrumentation(dir.path(), &tree);
        assert_eq!(output.files_with_instrumentation, 0);
        assert_eq!(output.total_source_files, 0);
    }

    #[test]
    fn detect_instrumentation_finds_rust_tracing() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("lib.rs"),
            "use tracing::info;\nfn f() { tracing::info!(\"hello\"); }",
        )
        .expect("write");

        let tree = TreeWalkerOutput {
            directories: vec![DirectoryEntry {
                path: ".".to_string(),
                source_files: vec![FileEntry {
                    path: "lib.rs".to_string(),
                    language: "Rust".to_string(),
                    signature: Vec::new(),
                    line_count: 2,
                }],
                test_files: Vec::new(),
            }],
            language_summary: BTreeMap::new(),
            total_source_files: 1,
            total_test_files: 0,
            excluded_directories: Vec::new(),
        };

        let output = detect_instrumentation(dir.path(), &tree);
        assert_eq!(output.files_with_instrumentation, 1);
        assert!(output
            .frameworks_detected
            .iter()
            .any(|f| f.contains("tracing")));
    }

    #[test]
    fn detect_instrumentation_ratio() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("a.rs"), "fn a() {}").expect("write");
        fs::write(
            dir.path().join("b.rs"),
            "fn b() { tracing::info!(\"hi\"); }",
        )
        .expect("write");

        let tree = TreeWalkerOutput {
            directories: vec![DirectoryEntry {
                path: ".".to_string(),
                source_files: vec![
                    FileEntry {
                        path: "a.rs".to_string(),
                        language: "Rust".to_string(),
                        signature: Vec::new(),
                        line_count: 1,
                    },
                    FileEntry {
                        path: "b.rs".to_string(),
                        language: "Rust".to_string(),
                        signature: Vec::new(),
                        line_count: 1,
                    },
                ],
                test_files: Vec::new(),
            }],
            language_summary: BTreeMap::new(),
            total_source_files: 2,
            total_test_files: 0,
            excluded_directories: Vec::new(),
        };

        let output = detect_instrumentation(dir.path(), &tree);
        assert_eq!(output.files_with_instrumentation, 1);
        assert_eq!(output.total_source_files, 2);
        assert!((output.instrumentation_ratio - 0.5).abs() < 0.01);
    }
}
