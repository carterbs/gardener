use crate::runtime::Terminal;
use crate::types::NonInteractiveReason;
use std::collections::BTreeMap;

pub type EnvMap = BTreeMap<String, String>;

pub fn is_non_interactive(
    env: &EnvMap,
    terminal: &dyn Terminal,
) -> Option<NonInteractiveReason> {
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

