#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use toml::Value;

const MANIFEST_RELATIVE_PATH: &str = "tools/gardener/testability-boundaries.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    UnitCore,
    BoundaryOrchestration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentationPolicy {
    Required,
    Exempt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub path: String,
    pub role: Role,
    pub owning_tests: Vec<String>,
    pub boundary_modes: Vec<String>,
    pub instrumentation: InstrumentationPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryManifest {
    pub repo_root: PathBuf,
    pub scope_roots: Vec<String>,
    pub allowlisted_paths: BTreeSet<String>,
    pub entries: Vec<ManifestEntry>,
    entry_by_path: BTreeMap<String, usize>,
}

impl BoundaryManifest {
    pub fn load() -> Self {
        let repo_root = repo_root();
        let manifest_path = repo_root.join(MANIFEST_RELATIVE_PATH);
        let source = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", manifest_path.display()));
        let value = source
            .parse::<Value>()
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", manifest_path.display()));

        let scope = value
            .get("scope")
            .and_then(Value::as_table)
            .unwrap_or_else(|| panic!("{} is missing [scope]", MANIFEST_RELATIVE_PATH));

        let scope_roots = read_string_list(scope, "roots");
        let allowlisted_paths = read_string_list(scope, "allowlisted_paths")
            .into_iter()
            .collect::<BTreeSet<_>>();

        let files = value
            .get("file")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{} is missing [[file]] entries", MANIFEST_RELATIVE_PATH));

        let mut entries = Vec::new();
        let mut entry_by_path = BTreeMap::new();
        for file in files {
            let table = file
                .as_table()
                .unwrap_or_else(|| panic!("manifest [[file]] entry must be a table"));
            let path = read_required_string(table, "path");
            let role = match read_required_string(table, "role").as_str() {
                "unit-core" => Role::UnitCore,
                "boundary-orchestration" => Role::BoundaryOrchestration,
                other => panic!("unsupported role '{other}' in {MANIFEST_RELATIVE_PATH}"),
            };
            let owning_tests = read_optional_string_list(table, "owning_tests");
            let boundary_modes = read_optional_string_list(table, "boundary_modes");
            let instrumentation = match table.get("instrumentation").and_then(Value::as_str) {
                Some("required") => InstrumentationPolicy::Required,
                Some("exempt") | None => InstrumentationPolicy::Exempt,
                Some(other) => {
                    panic!("unsupported instrumentation value '{other}' in {MANIFEST_RELATIVE_PATH}")
                }
            };

            let index = entries.len();
            if entry_by_path.insert(path.clone(), index).is_some() {
                panic!("duplicate manifest entry for {path}");
            }
            entries.push(ManifestEntry {
                path,
                role,
                owning_tests,
                boundary_modes,
                instrumentation,
            });
        }

        Self {
            repo_root,
            scope_roots,
            allowlisted_paths,
            entries,
            entry_by_path,
        }
    }

    pub fn entry_for_path(&self, path: &str) -> Option<&ManifestEntry> {
        self.entry_by_path
            .get(path)
            .and_then(|index| self.entries.get(*index))
    }

    pub fn in_scope_paths(&self) -> Vec<String> {
        let mut files = tracked_rust_files(&self.repo_root);
        files.retain(|path| self.scope_roots.iter().any(|root| path.starts_with(root)));
        files.sort();
        files.dedup();
        files
    }

    pub fn boundary_entries(&self) -> impl Iterator<Item = &ManifestEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.role == Role::BoundaryOrchestration)
    }

    pub fn unit_core_entries(&self) -> impl Iterator<Item = &ManifestEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.role == Role::UnitCore)
    }
}

pub fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("crate should live under <repo_root>/tools/gardener")
        .to_path_buf()
}

pub fn read_repo_file(relative_path: &str) -> String {
    fs::read_to_string(repo_root().join(relative_path))
        .unwrap_or_else(|err| panic!("failed to read {relative_path}: {err}"))
}

pub fn strip_comments_and_literals(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        let current = bytes[index];
        let next = bytes.get(index + 1).copied();

        if current == b'/' && next == Some(b'/') {
            output.push(' ');
            output.push(' ');
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                output.push(' ');
                index += 1;
            }
            continue;
        }

        if current == b'/' && next == Some(b'*') {
            output.push(' ');
            output.push(' ');
            index += 2;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    output.push(' ');
                    output.push(' ');
                    index += 2;
                    break;
                }
                output.push(if bytes[index] == b'\n' { '\n' } else { ' ' });
                index += 1;
            }
            continue;
        }

        if current == b'"' {
            output.push(' ');
            index += 1;
            while index < bytes.len() {
                let ch = bytes[index];
                if ch == b'\\' {
                    output.push(' ');
                    index += 1;
                    if index < bytes.len() {
                        output.push(if bytes[index] == b'\n' { '\n' } else { ' ' });
                        index += 1;
                    }
                    continue;
                }
                output.push(if ch == b'\n' { '\n' } else { ' ' });
                index += 1;
                if ch == b'"' {
                    break;
                }
            }
            continue;
        }

        if current == b'\'' {
            output.push(' ');
            index += 1;
            while index < bytes.len() {
                let ch = bytes[index];
                output.push(if ch == b'\n' { '\n' } else { ' ' });
                index += 1;
                if ch == b'\'' {
                    break;
                }
                if ch == b'\\' && index < bytes.len() {
                    output.push(if bytes[index] == b'\n' { '\n' } else { ' ' });
                    index += 1;
                }
            }
            continue;
        }

        output.push(current as char);
        index += 1;
    }

    output
}

pub fn module_path_for(relative_path: &str) -> String {
    let without_prefix = relative_path
        .strip_prefix("tools/gardener/src/")
        .unwrap_or(relative_path)
        .strip_suffix(".rs")
        .unwrap_or(relative_path);
    let segments = without_prefix
        .split('/')
        .filter(|segment| *segment != "mod")
        .collect::<Vec<_>>();
    format!("crate::{}", segments.join("::"))
}

fn read_required_string(table: &toml::map::Map<String, Value>, key: &str) -> String {
    table
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("manifest key '{key}' must be a string"))
        .to_string()
}

fn read_string_list(table: &toml::map::Map<String, Value>, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("manifest key '{key}' must be an array"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("manifest key '{key}' must contain strings"))
                .to_string()
        })
        .collect()
}

fn read_optional_string_list(table: &toml::map::Map<String, Value>, key: &str) -> Vec<String> {
    table
        .get(key)
        .map(|_| read_string_list(table, key))
        .unwrap_or_default()
}

fn tracked_rust_files(repo_root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["ls-files", "--", "*.rs"])
        .current_dir(repo_root)
        .output()
        .unwrap_or_else(|err| panic!("failed to run git ls-files: {err}"));
    if !output.status.success() {
        panic!(
            "git ls-files failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout)
        .expect("git ls-files output should be utf8")
        .lines()
        .map(|line| line.trim().replace('\\', "/"))
        .filter(|line| !line.is_empty())
        .collect()
}
