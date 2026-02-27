use crate::runtime::FileSystem;
use crate::runtime::Terminal;
use crate::types::NonInteractiveReason;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub type EnvMap = BTreeMap<String, String>;

pub fn is_non_interactive(env: &EnvMap, terminal: &dyn Terminal) -> Option<NonInteractiveReason> {
    if env.contains_key("CLAUDECODE") {
        return Some(NonInteractiveReason::ClaudeCodeEnv);
    }
    if env.contains_key("CODEX_THREAD_ID") {
        return Some(NonInteractiveReason::CodexThreadEnv);
    }
    if env.contains_key("CI") {
        return Some(NonInteractiveReason::CiEnv);
    }
    if !terminal.stdin_is_tty() {
        return Some(NonInteractiveReason::NonTtyStdin);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedAgent {
    Claude,
    Codex,
    Both,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentDetection {
    pub detected: DetectedAgent,
    pub claude_signals: Vec<String>,
    pub codex_signals: Vec<String>,
    pub agents_md_present: bool,
}

impl Default for DetectedAgent {
    fn default() -> Self {
        Self::Unknown
    }
}

pub fn detect_agent(fs: &dyn FileSystem, working_dir: &Path, repo_root: &Path) -> AgentDetection {
    let mut detection = AgentDetection::default();
    let roots = unique_roots(working_dir, repo_root);

    for root in roots {
        scan_agent_signals(fs, &root, &mut detection);
    }

    detection.detected = match (
        !detection.claude_signals.is_empty(),
        !detection.codex_signals.is_empty(),
    ) {
        (true, true) => DetectedAgent::Both,
        (true, false) => DetectedAgent::Claude,
        (false, true) => DetectedAgent::Codex,
        (false, false) => DetectedAgent::Unknown,
    };

    detection
}

fn unique_roots(working_dir: &Path, repo_root: &Path) -> Vec<PathBuf> {
    if working_dir == repo_root {
        return vec![repo_root.to_path_buf()];
    }
    vec![working_dir.to_path_buf(), repo_root.to_path_buf()]
}

fn scan_agent_signals(fs: &dyn FileSystem, root: &Path, output: &mut AgentDetection) {
    let is_root_note = |path: &str| -> String {
        if root.ends_with(path.trim_start_matches("./")) {
            path.to_string()
        } else {
            format!("{}:{}", root.display(), path)
        }
    };

    let claude_checks = [
        ".claude",
        "CLAUDE.md",
        ".claude/settings.json",
        ".claude/mcp.json",
        ".claude/skills",
        ".claude/commands",
    ];
    let codex_checks = [".codex/config.toml", "AGENTS.override.md"];

    for check in claude_checks {
        if fs.exists(&root.join(check)) {
            output.claude_signals.push(is_root_note(check));
        }
    }
    for check in codex_checks {
        if fs.exists(&root.join(check)) {
            output.codex_signals.push(is_root_note(check));
        }
    }

    if fs.exists(&root.join("AGENTS.md")) {
        output.agents_md_present = true;
        output.claude_signals.push(is_root_note("AGENTS.md"));
        output.codex_signals.push(is_root_note("AGENTS.md"));
    }
}
