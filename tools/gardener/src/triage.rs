use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::repo_intelligence::{
    build_profile, commits_since_profile_head, current_head_sha, read_profile, write_profile,
    RepoIntelligenceProfile,
};
use crate::runtime::ProductionRuntime;
use crate::triage_agent_detection::{detect_agent, is_non_interactive, DetectedAgent, EnvMap};
use crate::triage_discovery::{run_discovery, DiscoveryAssessment};
use crate::triage_interview::run_interview;
use crate::types::{AgentKind, RuntimeScope};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageDecision {
    Needed,
    NotNeeded,
}

pub fn profile_path(scope: &RuntimeScope, cfg: &AppConfig) -> PathBuf {
    let configured = PathBuf::from(&cfg.triage.output_path);
    if configured.is_absolute() {
        configured
    } else {
        scope
            .repo_root
            .as_ref()
            .unwrap_or(&scope.working_dir)
            .join(configured)
    }
}

pub fn triage_needed(
    scope: &RuntimeScope,
    cfg: &AppConfig,
    runtime: &ProductionRuntime,
    force_retriage: bool,
) -> Result<TriageDecision, GardenerError> {
    let path = profile_path(scope, cfg);
    if force_retriage || !runtime.file_system.exists(&path) {
        return Ok(TriageDecision::Needed);
    }

    let existing = read_profile(runtime.file_system.as_ref(), &path)?;
    let head = current_head_sha(runtime.process_runner.as_ref(), &scope.working_dir)
        .unwrap_or_else(|_| "unknown".to_string());
    if existing.meta.head_sha == head {
        return Ok(TriageDecision::NotNeeded);
    }
    let commits_since = commits_since_profile_head(
        runtime.process_runner.as_ref(),
        &scope.working_dir,
        &existing.meta.head_sha,
    )
    .unwrap_or(0);
    if commits_since > cfg.triage.stale_after_commits {
        Ok(TriageDecision::Needed)
    } else {
        Ok(TriageDecision::NotNeeded)
    }
}

pub fn run_triage(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    env: &EnvMap,
    agent_override: Option<AgentKind>,
) -> Result<RepoIntelligenceProfile, GardenerError> {
    if let Some(reason) = is_non_interactive(env, runtime.terminal.as_ref()) {
        return Err(GardenerError::Cli(format!(
            "Triage requires a human and cannot run non-interactively ({reason:?}).\nNo repo intelligence profile was found at {}.\nTo complete setup, run in a terminal:\n  gardener --triage-only",
            cfg.triage.output_path
        )));
    }

    let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);
    let detected = detect_agent(runtime.file_system.as_ref(), &scope.working_dir, repo_root);
    let chosen_agent = agent_override.unwrap_or_else(|| match detected.detected {
        DetectedAgent::Claude => AgentKind::Claude,
        _ => AgentKind::Codex,
    });

    if !runtime.terminal.stdin_is_tty() {
        runtime
            .terminal
            .write_line("━━━ Agent Detection ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        runtime
            .terminal
            .write_line(&format!("Detected agent: {:?}", detected.detected))?;
    }

    let discovery = run_discovery(
        runtime.process_runner.as_ref(),
        scope,
        chosen_agent,
        &cfg.seeding.model,
        cfg.triage.discovery_max_turns,
    )
    .unwrap_or_else(|_| DiscoveryAssessment::unknown());
    let discovery_used = discovery.agent_steering.grade != "unknown";

    let interview = run_interview(
        runtime.terminal.as_ref(),
        &discovery,
        cfg.orchestrator.parallelism,
        &cfg.validation.command,
    )?;

    let agents_md = detected.agents_md_present;
    let mut profile = build_profile(
        runtime.clock.as_ref(),
        &scope.working_dir,
        repo_root,
        current_head_sha(runtime.process_runner.as_ref(), &scope.working_dir)
            .unwrap_or_else(|_| "unknown".to_string()),
        discovery,
        discovery_used,
        Some(chosen_agent),
        detected.claude_signals,
        detected.codex_signals,
        interview.validation_command,
        agents_md,
    );
    profile.user_validated.additional_context = interview.additional_context;
    profile.user_validated.external_docs_accessible = interview.external_docs_accessible;
    profile.user_validated.preferred_parallelism = interview.preferred_parallelism;
    profile.user_validated.agent_steering_correction = interview.agent_steering_correction;
    profile.user_validated.external_docs_surface = interview.external_docs_surface;
    profile.user_validated.guardrails_correction = interview.guardrails_correction;
    profile.user_validated.coverage_grade_override = interview.coverage_grade_override;
    let path = profile_path(scope, cfg);
    write_profile(runtime.file_system.as_ref(), &path, &profile)?;
    Ok(profile)
}

pub fn ensure_profile_for_run(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    env: &EnvMap,
    force_retriage: bool,
    agent_override: Option<AgentKind>,
) -> Result<Option<RepoIntelligenceProfile>, GardenerError> {
    match triage_needed(scope, cfg, runtime, force_retriage)? {
        TriageDecision::NotNeeded => {
            let path = profile_path(scope, cfg);
            let profile = read_profile(runtime.file_system.as_ref(), &path)?;
            Ok(Some(profile))
        }
        TriageDecision::Needed => {
            if is_non_interactive(env, runtime.terminal.as_ref()).is_some() {
                return Err(GardenerError::Cli(format!(
                    "Triage requires a human and cannot run non-interactively.\n\nNo repo intelligence profile was found at {}.\nTriage gathers context that Gardener cannot determine automatically.\n\nTo complete setup, run in a terminal:\n  gardener --triage-only\n\nThen re-run your agent or pipeline.",
                    cfg.triage.output_path
                )));
            }
            run_triage(runtime, scope, cfg, env, agent_override).map(Some)
        }
    }
}
