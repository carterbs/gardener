use crate::errors::GardenerError;
use crate::quality_assertion_counter::{count_assertions, AssertionCounterOutput};
use crate::quality_ci_lint_detector::{detect_ci_lint, CiLintDetectorOutput};
use crate::quality_coverage_parser::{parse_coverage, CoverageParserOutput};
use crate::quality_debt_scanner::{scan_debt, DebtScannerOutput};
use crate::quality_doc_scanner::{scan_docs, DocScannerOutput};
use crate::quality_instrumentation_detector::{
    detect_instrumentation, InstrumentationDetectorOutput,
};
use crate::quality_test_detector::{detect_tests, TestDetectorOutput};
use crate::quality_tree_walker::{walk_repo, TreeWalkerOutput};
use crate::quality_untested_finder::{find_untested, UntestedFinderOutput};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceBundle {
    pub schema_version: u32,
    pub repo_path: String,
    pub collected_at: String,
    pub truncated: bool,
    pub files_included: usize,
    pub files_total: usize,
    pub tree: TreeWalkerOutput,
    pub tests: TestDetectorOutput,
    pub assertions: AssertionCounterOutput,
    pub coverage: CoverageParserOutput,
    pub untested: UntestedFinderOutput,
    pub debt: DebtScannerOutput,
    pub docs: DocScannerOutput,
    pub ci_lint: CiLintDetectorOutput,
    pub instrumentation: InstrumentationDetectorOutput,
    pub domain_hints: Option<DomainHints>,
    pub package_manifests: Vec<PackageManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainHints {
    pub domains: Vec<DomainHintEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainHintEntry {
    pub name: String,
    pub paths: Vec<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    pub path: String,
    pub manifest_type: String,
    pub name: Option<String>,
}

/// Collect a full evidence bundle for a repository.
///
/// This orchestrates all quality tools in sequence:
/// 1. Tree walker (enumerate files)
/// 2. Test detector (classify test files)
/// 3. Assertion counter (count assertions in tests)
/// 4. Coverage parser (parse coverage artifacts)
/// 5. Untested finder (identify untested source files)
/// 6. Debt scanner (find TODO/FIXME/HACK markers)
/// 7. Doc scanner (find steering and convention docs)
/// 8. CI/lint detector (find CI and linter configs)
/// 9. Instrumentation detector (find logging/tracing)
/// 10. Domain hints (from .gardener/domains.toml)
/// 11. Package manifests (Cargo.toml, package.json, etc.)
pub fn collect_evidence_bundle(repo_path: &Path) -> Result<EvidenceBundle, GardenerError> {
    let collected_at = chrono::Utc::now().to_rfc3339();

    // 1. Walk the repo tree
    let tree = walk_repo(repo_path);

    // 2. Detect test files
    let tests = detect_tests(repo_path, &tree);

    // 3. Count assertions in test files
    let assertions = count_assertions(repo_path, &tests);

    // 4. Parse coverage artifacts
    let coverage = parse_coverage(repo_path);

    // 5. Find untested source files
    let untested = find_untested(repo_path, &tree);

    // 6. Scan for debt markers
    let debt = scan_debt(repo_path, &tree);

    // 7. Scan for docs
    let docs = scan_docs(repo_path);

    // 8. Detect CI/lint config
    let ci_lint = detect_ci_lint(repo_path);

    // 9. Detect instrumentation
    let instrumentation = detect_instrumentation(repo_path, &tree);

    // 10. Load domain hints
    let domain_hints = load_domain_hints(repo_path);

    // 11. Detect package manifests
    let package_manifests = detect_package_manifests(repo_path);

    let files_total = tree.total_source_files + tree.total_test_files;
    let files_included = files_total; // No truncation in v1

    Ok(EvidenceBundle {
        schema_version: 1,
        repo_path: repo_path.to_string_lossy().to_string(),
        collected_at,
        truncated: false,
        files_included,
        files_total,
        tree,
        tests,
        assertions,
        coverage,
        untested,
        debt,
        docs,
        ci_lint,
        instrumentation,
        domain_hints,
        package_manifests,
    })
}

/// Load domain hints from `.gardener/domains.toml` if it exists.
fn load_domain_hints(repo_path: &Path) -> Option<DomainHints> {
    let hints_path = repo_path.join(".gardener/domains.toml");
    let content = std::fs::read_to_string(&hints_path).ok()?;

    // Parse the TOML content
    let parsed: toml::Value = toml::from_str(&content).ok()?;
    let domains_table = parsed.get("domain")?.as_table()?;

    let mut domains = Vec::new();
    for (name, value) in domains_table {
        let paths = value
            .get("paths")
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let description = value
            .get("description")
            .and_then(|d| d.as_str())
            .map(|s| s.to_string());

        domains.push(DomainHintEntry {
            name: name.clone(),
            paths,
            description,
        });
    }

    if domains.is_empty() {
        None
    } else {
        Some(DomainHints { domains })
    }
}

/// Detect package manifests (Cargo.toml, package.json, Package.swift, pyproject.toml, go.mod).
fn detect_package_manifests(repo_path: &Path) -> Vec<PackageManifest> {
    let manifest_files: &[(&str, &str)] = &[
        ("Cargo.toml", "cargo"),
        ("package.json", "npm"),
        ("Package.swift", "swift"),
        ("pyproject.toml", "python"),
        ("go.mod", "go"),
        ("setup.py", "python"),
        ("setup.cfg", "python"),
        ("Gemfile", "ruby"),
        ("pom.xml", "maven"),
        ("build.gradle", "gradle"),
        ("build.gradle.kts", "gradle"),
    ];

    let mut manifests = Vec::new();

    for (filename, manifest_type) in manifest_files {
        let full_path = repo_path.join(filename);
        if full_path.is_file() {
            let name = extract_manifest_name(&full_path, manifest_type);
            manifests.push(PackageManifest {
                path: filename.to_string(),
                manifest_type: manifest_type.to_string(),
                name,
            });
        }
    }

    // Also check for workspace members / monorepo packages
    scan_nested_manifests(repo_path, &mut manifests);

    manifests
}

/// Expand a workspace glob pattern (e.g., "packages/*") into concrete directory paths.
/// For literal paths, returns a single-element vec.
fn expand_workspace_glob(repo_path: &Path, pattern: &str) -> Vec<String> {
    if pattern.contains('*') {
        // Split on the first glob segment
        let parts: Vec<&str> = pattern.splitn(2, '*').collect();
        let prefix = parts[0].trim_end_matches('/');
        let base_dir = if prefix.is_empty() {
            repo_path.to_path_buf()
        } else {
            repo_path.join(prefix)
        };
        if !base_dir.is_dir() {
            return Vec::new();
        }
        let mut results = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&base_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let dir_name = entry.file_name().to_string_lossy().to_string();
                    let ws_path = if prefix.is_empty() {
                        dir_name
                    } else {
                        format!("{prefix}/{dir_name}")
                    };
                    results.push(ws_path);
                }
            }
        }
        results.sort();
        results
    } else {
        vec![pattern.to_string()]
    }
}

fn extract_manifest_name(path: &Path, manifest_type: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    match manifest_type {
        "cargo" => {
            // Parse [package] name from Cargo.toml
            let parsed: toml::Value = toml::from_str(&content).ok()?;
            parsed
                .get("package")?
                .get("name")?
                .as_str()
                .map(|s| s.to_string())
        }
        "npm" => {
            // Parse "name" from package.json
            let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
            parsed.get("name")?.as_str().map(|s| s.to_string())
        }
        "go" => {
            // First line of go.mod: "module github.com/foo/bar"
            content
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("module "))
                .map(|m| m.trim().to_string())
        }
        "python" => {
            // Try to find name in pyproject.toml
            let parsed: toml::Value = toml::from_str(&content).ok()?;
            parsed
                .get("project")
                .or_else(|| parsed.get("tool").and_then(|t| t.get("poetry")))
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        }
        _ => None,
    }
}

fn scan_nested_manifests(repo_path: &Path, manifests: &mut Vec<PackageManifest>) {
    // Check for Cargo workspace members
    let cargo_toml = repo_path.join("Cargo.toml");
    if cargo_toml.is_file() {
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            if let Ok(parsed) = toml::from_str::<toml::Value>(&content) {
                if let Some(members) = parsed
                    .get("workspace")
                    .and_then(|w| w.get("members"))
                    .and_then(|m| m.as_array())
                {
                    for member in members {
                        if let Some(member_path) = member.as_str() {
                            // member_path could be a glob like "crates/*"
                            // For simple paths, check directly
                            let member_cargo = repo_path.join(member_path).join("Cargo.toml");
                            if member_cargo.is_file() {
                                let rel_path = format!("{member_path}/Cargo.toml");
                                if !manifests.iter().any(|m| m.path == rel_path) {
                                    let name = extract_manifest_name(&member_cargo, "cargo");
                                    manifests.push(PackageManifest {
                                        path: rel_path,
                                        manifest_type: "cargo".to_string(),
                                        name,
                                    });
                                }
                            }
                            // For glob patterns, try to expand
                            if member_path.contains('*') {
                                let base =
                                    member_path.trim_end_matches("/*").trim_end_matches("/*");
                                let base_dir = repo_path.join(base);
                                if base_dir.is_dir() {
                                    if let Ok(entries) = std::fs::read_dir(&base_dir) {
                                        for entry in entries.flatten() {
                                            let p = entry.path();
                                            if p.is_dir() {
                                                let nested = p.join("Cargo.toml");
                                                if nested.is_file() {
                                                    let rel = nested
                                                        .strip_prefix(repo_path)
                                                        .ok()
                                                        .and_then(|r| r.to_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    if !manifests.iter().any(|m| m.path == rel) {
                                                        let name =
                                                            extract_manifest_name(&nested, "cargo");
                                                        manifests.push(PackageManifest {
                                                            path: rel,
                                                            manifest_type: "cargo".to_string(),
                                                            name,
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Check for npm workspaces
    let package_json = repo_path.join("package.json");
    if package_json.is_file() {
        if let Ok(content) = std::fs::read_to_string(&package_json) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                // workspaces can be an array of strings/globs, or an object with a "packages" array
                let ws_entries: Vec<String> = match parsed.get("workspaces") {
                    Some(serde_json::Value::Array(arr)) => arr
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    Some(serde_json::Value::Object(obj)) => obj
                        .get("packages")
                        .and_then(|p| p.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };

                for ws_pattern in &ws_entries {
                    // Expand glob patterns (e.g., "packages/*")
                    let resolved = expand_workspace_glob(repo_path, ws_pattern);
                    for ws_path in &resolved {
                        let pkg = repo_path.join(ws_path).join("package.json");
                        if pkg.is_file() {
                            let rel_path = format!("{ws_path}/package.json");
                            if !manifests.iter().any(|m| m.path == rel_path) {
                                let name = extract_manifest_name(&pkg, "npm");
                                manifests.push(PackageManifest {
                                    path: rel_path,
                                    manifest_type: "npm".to_string(),
                                    name,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn collect_evidence_bundle_empty_repo() {
        let dir = tempdir().expect("tempdir");
        let bundle = collect_evidence_bundle(dir.path()).expect("should succeed");
        assert_eq!(bundle.schema_version, 1);
        assert_eq!(bundle.files_total, 0);
        assert!(!bundle.truncated);
    }

    #[test]
    fn collect_evidence_bundle_with_rust_files() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("create dir");
        fs::write(
            src.join("lib.rs"),
            "fn main() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_works() {\n        assert_eq!(2 + 2, 4);\n    }\n}\n",
        )
        .expect("write");

        let bundle = collect_evidence_bundle(dir.path()).expect("should succeed");
        assert!(bundle.files_total > 0);
        assert!(
            !bundle.tests.test_files.is_empty() || !bundle.tests.untested_source_files.is_empty()
        );
    }

    #[test]
    fn detect_package_manifests_finds_cargo_toml() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
        )
        .expect("write");

        let manifests = detect_package_manifests(dir.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].manifest_type, "cargo");
        assert_eq!(manifests[0].name.as_deref(), Some("my-crate"));
    }

    #[test]
    fn load_domain_hints_returns_none_when_missing() {
        let dir = tempdir().expect("tempdir");
        assert!(load_domain_hints(dir.path()).is_none());
    }

    #[test]
    fn load_domain_hints_parses_toml() {
        let dir = tempdir().expect("tempdir");
        let gardener_dir = dir.path().join(".gardener");
        fs::create_dir_all(&gardener_dir).expect("create dir");
        fs::write(
            gardener_dir.join("domains.toml"),
            r#"
[domain.auth]
paths = ["src/auth/", "lib/auth/"]
description = "Authentication module"

[domain.api]
paths = ["src/api/"]
"#,
        )
        .expect("write");

        let hints = load_domain_hints(dir.path()).expect("should parse");
        assert_eq!(hints.domains.len(), 2);
    }
}
