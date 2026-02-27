use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    pub fn parse_cli(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonInteractiveReason {
    ClaudeCodeEnv,
    CodexThreadEnv,
    CiEnv,
    NonTtyStdin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerState {
    Understand,
    Planning,
    Doing,
    Gitting,
    Reviewing,
    Merging,
    Seeding,
    Complete,
    Failed,
    Parked,
}

impl WorkerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Understand => "understand",
            Self::Planning => "planning",
            Self::Doing => "doing",
            Self::Gitting => "gitting",
            Self::Reviewing => "reviewing",
            Self::Merging => "merging",
            Self::Seeding => "seeding",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Parked => "parked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationCommandSource {
    CliOverride,
    ConfigValidation,
    StartupValidation,
    AutoDiscovery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCommandResolution {
    pub command: String,
    pub source: ValidationCommandSource,
    pub startup_validate_on_boot: bool,
    pub startup_validation_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeScope {
    pub process_cwd: PathBuf,
    pub repo_root: Option<PathBuf>,
    pub working_dir: PathBuf,
}
