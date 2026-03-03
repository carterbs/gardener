use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageDefinition {
    pub name: String,
    pub extensions: Vec<String>,
    pub source_globs: Vec<String>,
    pub test_file_indicators: Vec<TestFileIndicator>,
    pub assertion_patterns: Vec<String>,
    pub coverage_artifacts: Vec<String>,
    pub instrumentation_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TestFileIndicator {
    /// Match against file content, e.g. "#[cfg(test)]"
    ContentMatch(String),
    /// Match against file path, e.g. "*_test.go"
    PathPattern(String),
    /// Match against directory name, e.g. "__tests__/"
    DirectoryConvention(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestType {
    Unit,
    Integration,
    EndToEnd,
    Unknown,
}

impl TestType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TestType::Unit => "unit",
            TestType::Integration => "integration",
            TestType::EndToEnd => "e2e",
            TestType::Unknown => "unknown",
        }
    }
}

/// Classify a test file as unit, integration, e2e, or unknown based on path heuristics.
pub fn classify_test_type(path: &Path, _language: &LanguageDefinition) -> TestType {
    let path_str = path.to_string_lossy().to_ascii_lowercase();

    // e2e patterns
    if path_str.contains("e2e")
        || path_str.contains("end_to_end")
        || path_str.contains("end-to-end")
        || path_str.contains("cypress")
        || path_str.contains("playwright")
        || path_str.contains("selenium")
    {
        return TestType::EndToEnd;
    }

    // integration patterns
    if path_str.contains("integration")
        || path_str.contains("integ_test")
        || path_str.contains("contract")
    {
        return TestType::Integration;
    }

    // Rust: tests/ directory at repo root is typically integration tests
    if path_str.contains("/tests/") || path_str.starts_with("tests/") {
        return TestType::Integration;
    }

    // Go: files ending with _test.go in the same package are unit tests
    // Python: test_ prefix files alongside source are unit tests
    // For inline test modules (Rust #[cfg(test)]), they're unit tests
    TestType::Unit
}

/// Return the full built-in language registry.
pub fn builtin_registry() -> Vec<LanguageDefinition> {
    vec![
        rust_definition(),
        typescript_definition(),
        swift_definition(),
        python_definition(),
        go_definition(),
    ]
}

/// Identify a file's language by extension (and optionally by first line shebang).
pub fn identify_language(path: &Path, first_line: Option<&str>) -> String {
    // Check shebang first
    if let Some(line) = first_line {
        if line.starts_with("#!") {
            let lower = line.to_ascii_lowercase();
            if lower.contains("python") {
                return "Python".to_string();
            }
            if lower.contains("node") || lower.contains("ts-node") || lower.contains("deno") {
                return "TypeScript/JavaScript".to_string();
            }
        }
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "rs" => "Rust".to_string(),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => "TypeScript/JavaScript".to_string(),
        "swift" => "Swift".to_string(),
        "py" | "pyi" => "Python".to_string(),
        "go" => "Go".to_string(),
        _ => "Unknown".to_string(),
    }
}

/// Look up the language definition for a given language name.
pub fn definition_for_language(name: &str) -> Option<LanguageDefinition> {
    builtin_registry().into_iter().find(|d| d.name == name)
}

fn rust_definition() -> LanguageDefinition {
    LanguageDefinition {
        name: "Rust".to_string(),
        extensions: vec![".rs".to_string()],
        source_globs: vec!["**/*.rs".to_string()],
        test_file_indicators: vec![
            TestFileIndicator::ContentMatch("#[cfg(test)]".to_string()),
            TestFileIndicator::ContentMatch("#[test]".to_string()),
            TestFileIndicator::DirectoryConvention("tests/".to_string()),
        ],
        assertion_patterns: vec![
            "assert!".to_string(),
            "assert_eq!".to_string(),
            "assert_ne!".to_string(),
            "debug_assert!".to_string(),
            "debug_assert_eq!".to_string(),
        ],
        coverage_artifacts: vec![
            "tarpaulin-report.json".to_string(),
            "cobertura.xml".to_string(),
            "lcov.info".to_string(),
        ],
        instrumentation_patterns: vec![
            "tracing::".to_string(),
            "log::".to_string(),
            "tracing_subscriber".to_string(),
            "env_logger".to_string(),
            "append_run_log(".to_string(),
        ],
    }
}

fn typescript_definition() -> LanguageDefinition {
    LanguageDefinition {
        name: "TypeScript/JavaScript".to_string(),
        extensions: vec![
            ".ts".to_string(),
            ".tsx".to_string(),
            ".js".to_string(),
            ".jsx".to_string(),
            ".mjs".to_string(),
            ".cjs".to_string(),
        ],
        source_globs: vec![
            "**/*.ts".to_string(),
            "**/*.tsx".to_string(),
            "**/*.js".to_string(),
            "**/*.jsx".to_string(),
        ],
        test_file_indicators: vec![
            TestFileIndicator::ContentMatch("describe(".to_string()),
            TestFileIndicator::ContentMatch("it(".to_string()),
            TestFileIndicator::ContentMatch("test(".to_string()),
            TestFileIndicator::PathPattern("*.test.ts".to_string()),
            TestFileIndicator::PathPattern("*.spec.ts".to_string()),
            TestFileIndicator::PathPattern("*.test.js".to_string()),
            TestFileIndicator::PathPattern("*.spec.js".to_string()),
            TestFileIndicator::PathPattern("*.test.tsx".to_string()),
            TestFileIndicator::PathPattern("*.spec.tsx".to_string()),
            TestFileIndicator::DirectoryConvention("__tests__/".to_string()),
        ],
        assertion_patterns: vec![
            "expect(".to_string(),
            "assert(".to_string(),
            "assert.".to_string(),
            "chai.expect".to_string(),
            "toBe(".to_string(),
            "toEqual(".to_string(),
        ],
        coverage_artifacts: vec![
            "coverage/lcov.info".to_string(),
            "coverage/coverage-final.json".to_string(),
            "coverage/cobertura-coverage.xml".to_string(),
            ".nyc_output/coverage.json".to_string(),
        ],
        instrumentation_patterns: vec![
            "console.log".to_string(),
            "console.error".to_string(),
            "console.warn".to_string(),
            "winston".to_string(),
            "pino".to_string(),
            "bunyan".to_string(),
            "log4js".to_string(),
        ],
    }
}

fn swift_definition() -> LanguageDefinition {
    LanguageDefinition {
        name: "Swift".to_string(),
        extensions: vec![".swift".to_string()],
        source_globs: vec!["**/*.swift".to_string()],
        test_file_indicators: vec![
            TestFileIndicator::ContentMatch("XCTestCase".to_string()),
            TestFileIndicator::ContentMatch("func test".to_string()),
            TestFileIndicator::PathPattern("*Tests.swift".to_string()),
            TestFileIndicator::DirectoryConvention("Tests/".to_string()),
        ],
        assertion_patterns: vec![
            "XCTAssert".to_string(),
            "XCTAssertEqual".to_string(),
            "XCTAssertNil".to_string(),
            "XCTAssertNotNil".to_string(),
            "XCTAssertTrue".to_string(),
            "XCTAssertFalse".to_string(),
            "XCTFail".to_string(),
        ],
        coverage_artifacts: vec![
            "*.xcresult".to_string(),
            "codecov.json".to_string(),
            "lcov.info".to_string(),
        ],
        instrumentation_patterns: vec![
            "os_log".to_string(),
            "Logger(".to_string(),
            "OSLog".to_string(),
            "print(".to_string(),
            "NSLog(".to_string(),
        ],
    }
}

fn python_definition() -> LanguageDefinition {
    LanguageDefinition {
        name: "Python".to_string(),
        extensions: vec![".py".to_string(), ".pyi".to_string()],
        source_globs: vec!["**/*.py".to_string()],
        test_file_indicators: vec![
            TestFileIndicator::ContentMatch("def test_".to_string()),
            TestFileIndicator::ContentMatch("class Test".to_string()),
            TestFileIndicator::ContentMatch("unittest.TestCase".to_string()),
            TestFileIndicator::PathPattern("test_*.py".to_string()),
            TestFileIndicator::PathPattern("*_test.py".to_string()),
            TestFileIndicator::DirectoryConvention("tests/".to_string()),
            TestFileIndicator::DirectoryConvention("test/".to_string()),
        ],
        assertion_patterns: vec![
            "assert ".to_string(),
            "self.assert".to_string(),
            "self.assertEqual".to_string(),
            "self.assertTrue".to_string(),
            "self.assertFalse".to_string(),
            "self.assertRaises".to_string(),
            "pytest.raises".to_string(),
        ],
        coverage_artifacts: vec![
            "coverage.xml".to_string(),
            ".coverage".to_string(),
            "htmlcov/".to_string(),
            "lcov.info".to_string(),
        ],
        instrumentation_patterns: vec![
            "logging.".to_string(),
            "logger.".to_string(),
            "getLogger".to_string(),
            "structlog".to_string(),
            "loguru".to_string(),
        ],
    }
}

fn go_definition() -> LanguageDefinition {
    LanguageDefinition {
        name: "Go".to_string(),
        extensions: vec![".go".to_string()],
        source_globs: vec!["**/*.go".to_string()],
        test_file_indicators: vec![
            TestFileIndicator::ContentMatch("func Test".to_string()),
            TestFileIndicator::ContentMatch("func Benchmark".to_string()),
            TestFileIndicator::PathPattern("*_test.go".to_string()),
        ],
        assertion_patterns: vec![
            "t.Error".to_string(),
            "t.Fatal".to_string(),
            "t.Fail".to_string(),
            "require.".to_string(),
            "assert.".to_string(),
            "is.Equal".to_string(),
        ],
        coverage_artifacts: vec![
            "cover.out".to_string(),
            "coverage.out".to_string(),
            "coverage.txt".to_string(),
        ],
        instrumentation_patterns: vec![
            "log.".to_string(),
            "zap.".to_string(),
            "logrus.".to_string(),
            "zerolog".to_string(),
            "slog.".to_string(),
            "klog.".to_string(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn identify_language_by_extension() {
        assert_eq!(identify_language(Path::new("foo.rs"), None), "Rust");
        assert_eq!(
            identify_language(Path::new("bar.ts"), None),
            "TypeScript/JavaScript"
        );
        assert_eq!(identify_language(Path::new("baz.swift"), None), "Swift");
        assert_eq!(identify_language(Path::new("qux.py"), None), "Python");
        assert_eq!(identify_language(Path::new("main.go"), None), "Go");
        assert_eq!(identify_language(Path::new("data.csv"), None), "Unknown");
    }

    #[test]
    fn identify_language_by_shebang() {
        assert_eq!(
            identify_language(Path::new("script"), Some("#!/usr/bin/env python3")),
            "Python"
        );
        assert_eq!(
            identify_language(Path::new("run"), Some("#!/usr/bin/env node")),
            "TypeScript/JavaScript"
        );
    }

    #[test]
    fn builtin_registry_has_five_languages() {
        let registry = builtin_registry();
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn classify_integration_test_by_path() {
        let lang = rust_definition();
        assert_eq!(
            classify_test_type(Path::new("tests/integration.rs"), &lang),
            TestType::Integration
        );
    }

    #[test]
    fn classify_e2e_test_by_path() {
        let lang = typescript_definition();
        assert_eq!(
            classify_test_type(Path::new("e2e/login.test.ts"), &lang),
            TestType::EndToEnd
        );
    }

    #[test]
    fn classify_unit_test_by_default() {
        let lang = rust_definition();
        assert_eq!(
            classify_test_type(Path::new("src/lib.rs"), &lang),
            TestType::Unit
        );
    }
}
