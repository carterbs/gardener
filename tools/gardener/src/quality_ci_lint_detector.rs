use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiLintDetectorOutput {
    pub ci: ToolPresence,
    pub linters: ToolPresence,
    pub pre_commit: ToolPresence,
    pub coverage_thresholds: ToolPresence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPresence {
    pub detected: bool,
    pub files: Vec<String>,
    pub details: Vec<String>,
}

/// CI configuration paths to check.
const CI_CONFIG_PATHS: &[&str] = &[
    ".github/workflows",
    ".gitlab-ci.yml",
    ".circleci/config.yml",
    "Jenkinsfile",
    ".travis.yml",
    "azure-pipelines.yml",
    ".buildkite/pipeline.yml",
    "bitbucket-pipelines.yml",
    ".drone.yml",
    "Taskfile.yml",
    "Makefile",
];

/// Linter configuration paths to check.
const LINTER_CONFIG_PATHS: &[&str] = &[
    ".eslintrc",
    ".eslintrc.js",
    ".eslintrc.json",
    ".eslintrc.yml",
    "eslint.config.js",
    "eslint.config.mjs",
    ".prettierrc",
    ".prettierrc.json",
    ".prettierrc.yml",
    "prettier.config.js",
    "rustfmt.toml",
    ".rustfmt.toml",
    "clippy.toml",
    ".clippy.toml",
    ".pylintrc",
    "pyproject.toml",
    "setup.cfg",
    ".flake8",
    ".golangci.yml",
    ".golangci.yaml",
    ".swiftlint.yml",
    ".swiftformat",
    "biome.json",
    "deno.json",
    ".stylelintrc",
    ".stylelintrc.json",
];

/// Pre-commit hook paths.
const PRE_COMMIT_PATHS: &[&str] = &[
    ".pre-commit-config.yaml",
    ".husky/pre-commit",
    ".husky/_/pre-commit",
    ".git/hooks/pre-commit",
    ".githooks/pre-commit",
    ".lefthook.yml",
    "lefthook.yml",
    ".lintstagedrc",
    ".lintstagedrc.json",
    "lint-staged.config.js",
];

/// Detect CI configs, linter configs, pre-commit hooks, and coverage thresholds.
pub fn detect_ci_lint(repo_path: &Path) -> CiLintDetectorOutput {
    let ci = detect_ci(repo_path);
    let linters = detect_linters(repo_path);
    let pre_commit = detect_pre_commit(repo_path);
    let coverage_thresholds = detect_coverage_thresholds(repo_path);

    CiLintDetectorOutput {
        ci,
        linters,
        pre_commit,
        coverage_thresholds,
    }
}

fn detect_ci(repo_path: &Path) -> ToolPresence {
    let mut files = Vec::new();
    let mut details = Vec::new();

    for config_path in CI_CONFIG_PATHS {
        let full = repo_path.join(config_path);
        if full.is_file() {
            files.push(config_path.to_string());
            details.push(format!("Found CI config: {config_path}"));
        } else if full.is_dir() {
            // For directories like .github/workflows, check for yaml files inside
            if let Ok(entries) = std::fs::read_dir(&full) {
                let yaml_files: Vec<String> = entries
                    .flatten()
                    .filter(|e| {
                        let p = e.path();
                        p.is_file()
                            && p.extension()
                                .and_then(|ext| ext.to_str())
                                .map(|ext| ext == "yml" || ext == "yaml")
                                .unwrap_or(false)
                    })
                    .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                    .collect();

                if !yaml_files.is_empty() {
                    files.push(config_path.to_string());
                    details.push(format!(
                        "Found {} workflow(s) in {config_path}",
                        yaml_files.len()
                    ));
                }
            }
        }
    }

    ToolPresence {
        detected: !files.is_empty(),
        files,
        details,
    }
}

fn detect_linters(repo_path: &Path) -> ToolPresence {
    let mut files = Vec::new();
    let mut details = Vec::new();

    for config_path in LINTER_CONFIG_PATHS {
        let full = repo_path.join(config_path);
        if full.is_file() {
            files.push(config_path.to_string());

            // Try to identify the linter type
            let linter_name = identify_linter(config_path);
            details.push(format!("Found {linter_name} config: {config_path}"));
        }
    }

    // Check Cargo.toml for clippy configuration
    let cargo_toml = repo_path.join("Cargo.toml");
    if cargo_toml.is_file() {
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            if content.contains("[lints") || content.contains("clippy") {
                if !files.iter().any(|f| f.contains("Cargo.toml")) {
                    files.push("Cargo.toml".to_string());
                    details.push("Clippy lints configured in Cargo.toml".to_string());
                }
            }
        }
    }

    // Check package.json for lint scripts
    let package_json = repo_path.join("package.json");
    if package_json.is_file() {
        if let Ok(content) = std::fs::read_to_string(&package_json) {
            if content.contains("\"lint\"") || content.contains("\"eslint\"") {
                details.push("Lint script found in package.json".to_string());
            }
        }
    }

    ToolPresence {
        detected: !files.is_empty(),
        files,
        details,
    }
}

fn detect_pre_commit(repo_path: &Path) -> ToolPresence {
    let mut files = Vec::new();
    let mut details = Vec::new();

    // Check well-known config paths
    for config_path in PRE_COMMIT_PATHS {
        let full = repo_path.join(config_path);
        if full.is_file() {
            files.push(config_path.to_string());
            details.push(format!("Found pre-commit config: {config_path}"));
        }
    }

    // Read git config to find custom hooksPath and check for active hooks there
    if let Some(hooks_dir) = resolve_git_hooks_path(repo_path) {
        let hook_names = ["pre-commit", "pre-push", "commit-msg"];
        for hook in &hook_names {
            let hook_path = hooks_dir.join(hook);
            if hook_path.is_file() {
                let relative = hook_path
                    .strip_prefix(repo_path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| hook_path.display().to_string());
                if !files.iter().any(|f| f == &relative) {
                    files.push(relative.clone());
                    details.push(format!("Active git hook: {relative}"));
                }
            }
        }
    }

    // Check package.json for husky/lint-staged
    let package_json = repo_path.join("package.json");
    if package_json.is_file() {
        if let Ok(content) = std::fs::read_to_string(&package_json) {
            if content.contains("\"husky\"") {
                files.push("package.json".to_string());
                details.push("Husky configured in package.json".to_string());
            }
            if content.contains("\"lint-staged\"") {
                if !files.iter().any(|f| f == "package.json") {
                    files.push("package.json".to_string());
                }
                details.push("lint-staged configured in package.json".to_string());
            }
        }
    }

    ToolPresence {
        detected: !files.is_empty(),
        files,
        details,
    }
}

/// Resolve the git hooks directory by checking `core.hooksPath` in git config.
///
/// Reads `.git/config` (or the gitdir for worktrees) to find a custom hooks path.
/// Falls back to `.git/hooks` if no custom path is configured.
fn resolve_git_hooks_path(repo_path: &Path) -> Option<PathBuf> {
    // For worktrees, .git may be a file pointing to the real gitdir
    let dot_git = repo_path.join(".git");
    let git_config_path = if dot_git.is_file() {
        // Worktree: .git is a file like "gitdir: /path/to/main/.git/worktrees/foo"
        // Read the main repo's config instead
        let content = std::fs::read_to_string(&dot_git).ok()?;
        let gitdir = content.strip_prefix("gitdir: ")?.trim();
        let gitdir_path = if Path::new(gitdir).is_absolute() {
            PathBuf::from(gitdir)
        } else {
            repo_path.join(gitdir)
        };
        // Walk up from .git/worktrees/foo to .git/config
        let main_git_dir = gitdir_path.parent()?.parent()?;
        main_git_dir.join("config")
    } else if dot_git.is_dir() {
        dot_git.join("config")
    } else {
        return None;
    };

    let config_content = std::fs::read_to_string(&git_config_path).ok()?;

    // Parse core.hooksPath from git config (ini-style)
    let hooks_path = parse_git_config_hooks_path(&config_content);

    match hooks_path {
        Some(custom_path) => {
            let p = Path::new(&custom_path);
            if p.is_absolute() {
                Some(p.to_path_buf())
            } else {
                Some(repo_path.join(p))
            }
        }
        None => {
            // Default: .git/hooks
            let default = repo_path.join(".git/hooks");
            if default.is_dir() {
                Some(default)
            } else {
                None
            }
        }
    }
}

/// Extract `hooksPath` from a `[core]` section in git config content.
fn parse_git_config_hooks_path(content: &str) -> Option<String> {
    let mut in_core = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_core = trimmed.eq_ignore_ascii_case("[core]");
            continue;
        }
        if in_core {
            if let Some(rest) = trimmed.strip_prefix("hooksPath") {
                let rest = rest.trim_start();
                if let Some(value) = rest.strip_prefix('=') {
                    return Some(value.trim().to_string());
                }
            }
        }
    }
    None
}

fn detect_coverage_thresholds(repo_path: &Path) -> ToolPresence {
    let mut files = Vec::new();
    let mut details = Vec::new();

    // Check jest.config for coverage thresholds
    for jest_config in &[
        "jest.config.js",
        "jest.config.ts",
        "jest.config.json",
        "jest.config.mjs",
    ] {
        let full = repo_path.join(jest_config);
        if full.is_file() {
            if let Ok(content) = std::fs::read_to_string(&full) {
                if content.contains("coverageThreshold") {
                    files.push(jest_config.to_string());
                    details.push(format!("Jest coverage threshold in {jest_config}"));
                }
            }
        }
    }

    // Check package.json for jest coverage thresholds
    let package_json = repo_path.join("package.json");
    if package_json.is_file() {
        if let Ok(content) = std::fs::read_to_string(&package_json) {
            if content.contains("coverageThreshold") {
                files.push("package.json".to_string());
                details.push("Jest coverage threshold in package.json".to_string());
            }
        }
    }

    // Check .codecov.yml for coverage targets
    for codecov_config in &[".codecov.yml", "codecov.yml", ".codecov.yaml"] {
        let full = repo_path.join(codecov_config);
        if full.is_file() {
            files.push(codecov_config.to_string());
            details.push(format!("Codecov config found: {codecov_config}"));
        }
    }

    // Check pyproject.toml for coverage settings
    let pyproject = repo_path.join("pyproject.toml");
    if pyproject.is_file() {
        if let Ok(content) = std::fs::read_to_string(&pyproject) {
            if content.contains("[tool.coverage") || content.contains("fail_under") {
                files.push("pyproject.toml".to_string());
                details.push("Coverage threshold in pyproject.toml".to_string());
            }
        }
    }

    // Check GitHub Actions workflows for coverage enforcement
    let workflows_dir = repo_path.join(".github/workflows");
    if workflows_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&workflows_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if content.contains("coverage") && content.contains("threshold") {
                            let name = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("unknown");
                            details.push(format!(
                                "Coverage enforcement in workflow: {name}"
                            ));
                        }
                    }
                }
            }
        }
    }

    ToolPresence {
        detected: !files.is_empty() || !details.is_empty(),
        files,
        details,
    }
}

fn identify_linter(config_path: &str) -> &str {
    if config_path.contains("eslint") {
        "ESLint"
    } else if config_path.contains("prettier") {
        "Prettier"
    } else if config_path.contains("rustfmt") {
        "rustfmt"
    } else if config_path.contains("clippy") {
        "Clippy"
    } else if config_path.contains("pylint") {
        "Pylint"
    } else if config_path.contains("flake8") {
        "Flake8"
    } else if config_path.contains("golangci") {
        "golangci-lint"
    } else if config_path.contains("swiftlint") {
        "SwiftLint"
    } else if config_path.contains("swiftformat") {
        "SwiftFormat"
    } else if config_path.contains("biome") {
        "Biome"
    } else if config_path.contains("stylelint") {
        "Stylelint"
    } else if config_path.contains("pyproject") {
        "Python tooling"
    } else if config_path.contains("setup.cfg") {
        "Python setup"
    } else {
        "linter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detect_ci_lint_empty_repo() {
        let dir = tempdir().expect("tempdir");
        let output = detect_ci_lint(dir.path());
        assert!(!output.ci.detected);
        assert!(!output.linters.detected);
        assert!(!output.pre_commit.detected);
        assert!(!output.coverage_thresholds.detected);
    }

    #[test]
    fn detect_ci_finds_github_workflows() {
        let dir = tempdir().expect("tempdir");
        let workflows = dir.path().join(".github/workflows");
        fs::create_dir_all(&workflows).expect("create dir");
        fs::write(workflows.join("ci.yml"), "name: CI\n").expect("write");
        let output = detect_ci_lint(dir.path());
        assert!(output.ci.detected);
    }

    #[test]
    fn detect_linters_finds_eslintrc() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join(".eslintrc.json"), "{}").expect("write");
        let output = detect_ci_lint(dir.path());
        assert!(output.linters.detected);
        assert!(output.linters.details[0].contains("ESLint"));
    }

    #[test]
    fn detect_pre_commit_finds_husky() {
        let dir = tempdir().expect("tempdir");
        let husky = dir.path().join(".husky");
        fs::create_dir_all(&husky).expect("create dir");
        fs::write(husky.join("pre-commit"), "#!/bin/sh\nnpx lint-staged\n").expect("write");
        let output = detect_ci_lint(dir.path());
        assert!(output.pre_commit.detected);
    }

    #[test]
    fn detect_pre_commit_finds_hooks_via_git_config() {
        let dir = tempdir().expect("tempdir");
        // Set up .git/config with core.hooksPath
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).expect("create .git");
        fs::write(
            git_dir.join("config"),
            "[core]\n\thooksPath = .githooks\n",
        )
        .expect("write git config");
        // Create the hooks directory with an active pre-commit
        let hooks = dir.path().join(".githooks");
        fs::create_dir_all(&hooks).expect("create .githooks");
        fs::write(hooks.join("pre-commit"), "#!/bin/sh\necho test\n").expect("write hook");
        let output = detect_ci_lint(dir.path());
        assert!(output.pre_commit.detected);
        assert!(output.pre_commit.files.iter().any(|f| f.contains(".githooks/pre-commit")));
    }

    #[test]
    fn parse_git_config_hooks_path_extracts_value() {
        let config = "[user]\n\tname = test\n[core]\n\thooksPath = .githooks\n[remote]\n";
        assert_eq!(
            parse_git_config_hooks_path(config),
            Some(".githooks".to_string())
        );
    }

    #[test]
    fn parse_git_config_hooks_path_returns_none_when_absent() {
        let config = "[core]\n\tautocrlf = false\n";
        assert_eq!(parse_git_config_hooks_path(config), None);
    }
}
