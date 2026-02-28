use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ClippyLintConfig {
    pub lints: Vec<LintRule>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LintRule {
    pub name: String,
    pub level: LintLevel,
    pub scope: LintScope,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LintLevel {
    Warn,
    Deny,
}

impl LintLevel {
    pub fn flag_prefix(&self) -> &'static str {
        match self {
            Self::Warn => "-W",
            Self::Deny => "-D",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum LintScope {
    AllTargets,
    LibBins,
}

impl LintScope {
    pub fn cargo_args(&self) -> &'static [&'static str] {
        match self {
            Self::AllTargets => &["--all-targets"],
            Self::LibBins => &["--lib", "--bins"],
        }
    }
}

impl ClippyLintConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let contents =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        toml::from_str(&contents).map_err(|e| format!("parse {}: {e}", path.display()))
    }

    pub fn flags_by_scope(&self) -> BTreeMap<LintScope, Vec<String>> {
        let mut grouped: BTreeMap<LintScope, Vec<String>> = BTreeMap::new();
        for rule in &self.lints {
            grouped
                .entry(rule.scope)
                .or_default()
                .push(format!("{} {}", rule.level.flag_prefix(), rule.name));
        }
        grouped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clippy_lints_toml() {
        let config: ClippyLintConfig = toml::from_str(
            r#"
            [[lints]]
            name = "clippy::unwrap_used"
            level = "warn"
            scope = "all-targets"

            [[lints]]
            name = "clippy::unwrap_used"
            level = "deny"
            scope = "lib-bins"
            "#,
        )
        .expect("should parse");

        assert_eq!(config.lints.len(), 2);
        assert_eq!(config.lints[0].level, LintLevel::Warn);
        assert_eq!(config.lints[0].scope, LintScope::AllTargets);
        assert_eq!(config.lints[1].level, LintLevel::Deny);
        assert_eq!(config.lints[1].scope, LintScope::LibBins);
    }

    #[test]
    fn flags_by_scope_groups_correctly() {
        let config = ClippyLintConfig {
            lints: vec![
                LintRule {
                    name: "clippy::unwrap_used".to_string(),
                    level: LintLevel::Warn,
                    scope: LintScope::AllTargets,
                },
                LintRule {
                    name: "clippy::expect_used".to_string(),
                    level: LintLevel::Warn,
                    scope: LintScope::AllTargets,
                },
                LintRule {
                    name: "clippy::unwrap_used".to_string(),
                    level: LintLevel::Deny,
                    scope: LintScope::LibBins,
                },
            ],
        };

        let grouped = config.flags_by_scope();
        assert_eq!(grouped.len(), 2);
        assert_eq!(
            grouped[&LintScope::AllTargets],
            vec!["-W clippy::unwrap_used", "-W clippy::expect_used"]
        );
        assert_eq!(
            grouped[&LintScope::LibBins],
            vec!["-D clippy::unwrap_used"]
        );
    }

    #[test]
    fn load_repo_clippy_lints_toml() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("clippy-lints.toml");
        let config = ClippyLintConfig::load(&path).expect("clippy-lints.toml should parse");
        assert!(
            !config.lints.is_empty(),
            "clippy-lints.toml should have at least one rule"
        );
    }

    #[test]
    fn lint_level_flag_prefix() {
        assert_eq!(LintLevel::Warn.flag_prefix(), "-W");
        assert_eq!(LintLevel::Deny.flag_prefix(), "-D");
    }

    #[test]
    fn lint_scope_cargo_args() {
        assert_eq!(LintScope::AllTargets.cargo_args(), &["--all-targets"]);
        assert_eq!(LintScope::LibBins.cargo_args(), &["--lib", "--bins"]);
    }
}
