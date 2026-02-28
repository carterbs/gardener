use crate::logging::append_run_log;
use crate::runtime::FileSystem;
use crate::runtime::Terminal;
use crate::types::NonInteractiveReason;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub type EnvMap = BTreeMap<String, String>;

pub fn is_non_interactive(env: &EnvMap, terminal: &dyn Terminal) -> Option<NonInteractiveReason> {
    if env.contains_key("CLAUDECODE") {
        append_run_log(
            "debug",
            "triage.agent_detection.non_interactive",
            json!({ "reason": "ClaudeCodeEnv" }),
        );
        return Some(NonInteractiveReason::ClaudeCodeEnv);
    }
    if env.contains_key("CODEX_THREAD_ID") {
        append_run_log(
            "debug",
            "triage.agent_detection.non_interactive",
            json!({ "reason": "CodexThreadEnv" }),
        );
        return Some(NonInteractiveReason::CodexThreadEnv);
    }
    if env.contains_key("CI") {
        append_run_log(
            "debug",
            "triage.agent_detection.non_interactive",
            json!({ "reason": "CiEnv" }),
        );
        return Some(NonInteractiveReason::CiEnv);
    }
    if !terminal.stdin_is_tty() {
        append_run_log(
            "debug",
            "triage.agent_detection.non_interactive",
            json!({ "reason": "NonTtyStdin" }),
        );
        return Some(NonInteractiveReason::NonTtyStdin);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetectedAgent {
    Claude,
    Codex,
    Both,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentDetection {
    pub detected: DetectedAgent,
    pub claude_signals: Vec<String>,
    pub codex_signals: Vec<String>,
    pub agents_md_present: bool,
}

pub fn detect_agent(fs: &dyn FileSystem, working_dir: &Path, repo_root: &Path) -> AgentDetection {
    let mut detection = AgentDetection::default();
    let roots = unique_roots(working_dir, repo_root);

    append_run_log(
        "debug",
        "triage.agent_detection.scanning",
        json!({
            "working_dir": working_dir.display().to_string(),
            "repo_root": repo_root.display().to_string(),
            "roots_count": roots.len()
        }),
    );

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

    append_run_log(
        "info",
        "triage.agent_detection.result",
        json!({
            "detected": format!("{:?}", detection.detected),
            "claude_signals": detection.claude_signals,
            "codex_signals": detection.codex_signals,
            "agents_md_present": detection.agents_md_present
        }),
    );

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
