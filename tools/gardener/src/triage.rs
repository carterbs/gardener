use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::repo_intelligence::{
    build_profile, commits_since_profile_head, current_head_sha, read_profile, write_profile,
    RepoIntelligenceProfile,
};
use crate::runtime::ProductionRuntime;
use crate::triage_agent_detection::{detect_agent, is_non_interactive, DetectedAgent, EnvMap};
use crate::triage_discovery::{run_discovery, DiscoveryAssessment};
use crate::triage_interview::run_interview;
use crate::types::{AgentKind, RuntimeScope};
use serde_json::json;
use std::env;
use std::path::PathBuf;

fn push_triage_update(
    runtime: &ProductionRuntime,
    activity: &mut Vec<String>,
    artifacts: &mut Vec<String>,
    activity_line: impl Into<String>,
    artifact_line: Option<String>,
) -> Result<(), GardenerError> {
    if !runtime.terminal.stdin_is_tty() {
        return Ok(());
    }
    activity.push(activity_line.into());
    if activity.len() > 10 {
        let drop = activity.len() - 10;
        activity.drain(0..drop);
    }
    if let Some(line) = artifact_line {
        artifacts.push(line);
    }
    runtime.terminal.draw_triage(activity, artifacts)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageDecision {
    Needed,
    NotNeeded,
}

fn default_repo_intelligence_path(scope: &RuntimeScope) -> PathBuf {
    let repo_name = scope
        .repo_root
        .as_ref()
        .or(Some(&scope.working_dir))
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("repo");

    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".gardener").join(repo_name))
        .unwrap_or_else(|| {
            scope
                .repo_root
                .as_ref()
                .unwrap_or(&scope.working_dir)
                .join(".gardener")
        })
        .join("repo-intelligence.toml")
}

pub fn profile_path(scope: &RuntimeScope, cfg: &AppConfig) -> PathBuf {
    let configured = PathBuf::from(&cfg.triage.output_path);
    if configured.is_absolute() {
        configured
    } else if cfg.triage.output_path == ".gardener/repo-intelligence.toml" {
        default_repo_intelligence_path(scope)
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
        append_run_log(
            "info",
            "triage.needed",
            json!({
                "reason": if force_retriage { "force_retriage" } else { "profile_missing" },
                "path": path.display().to_string()
            }),
        );
        return Ok(TriageDecision::Needed);
    }

    let existing = read_profile(runtime.file_system.as_ref(), &path)?;
    let head = current_head_sha(runtime.process_runner.as_ref(), &scope.working_dir)
        .unwrap_or_else(|_| "unknown".to_string());
    if existing.meta.head_sha == head {
        append_run_log(
            "debug",
            "triage.not_needed",
            json!({
                "reason": "head_sha_matches",
                "head_sha": head,
                "path": path.display().to_string()
            }),
        );
        return Ok(TriageDecision::NotNeeded);
    }
    let commits_since = commits_since_profile_head(
        runtime.process_runner.as_ref(),
        &scope.working_dir,
        &existing.meta.head_sha,
    )
    .unwrap_or(0);
    if commits_since > cfg.triage.stale_after_commits {
        append_run_log(
            "info",
            "triage.needed",
            json!({
                "reason": "profile_stale",
                "commits_since": commits_since,
                "stale_after_commits": cfg.triage.stale_after_commits,
                "profile_head_sha": existing.meta.head_sha,
                "current_head_sha": head
            }),
        );
        Ok(TriageDecision::Needed)
    } else {
        append_run_log(
            "debug",
            "triage.not_needed",
            json!({
                "reason": "within_staleness_threshold",
                "commits_since": commits_since,
                "stale_after_commits": cfg.triage.stale_after_commits
            }),
        );
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
    let mut activity = Vec::new();
    let mut artifacts = Vec::new();
    let resolved_profile_path = profile_path(scope, cfg);
    append_run_log(
        "info",
        "triage.started",
        json!({
            "working_dir": scope.working_dir.display().to_string(),
            "agent_override": agent_override.map(|a| a.as_str()),
            "output_path": resolved_profile_path.display().to_string(),
        }),
    );
    push_triage_update(
        runtime,
        &mut activity,
        &mut artifacts,
        "Starting triage session",
        None,
    )?;

    if let Some(reason) = is_non_interactive(env, runtime.terminal.as_ref()) {
        append_run_log(
            "error",
            "triage.non_interactive_rejected",
            json!({
                "reason": format!("{reason:?}")
            }),
        );
        return Err(GardenerError::Cli(format!(
            "Triage requires a human and cannot run non-interactively ({reason:?}).\nNo repo intelligence profile was found at {}.\nTo complete setup, run in a terminal:\n  gardener --triage-only",
            resolved_profile_path.display()
        )));
    }

    let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);
    push_triage_update(
        runtime,
        &mut activity,
        &mut artifacts,
        "Detecting coding agent signals",
        None,
    )?;
    let detected = detect_agent(runtime.file_system.as_ref(), &scope.working_dir, repo_root);
    let chosen_agent = agent_override.unwrap_or(match detected.detected {
        DetectedAgent::Claude => AgentKind::Claude,
        _ => AgentKind::Codex,
    });
    append_run_log(
        "info",
        "triage.agent.detected",
        json!({
            "detected": format!("{:?}", detected.detected),
            "chosen_agent": chosen_agent.as_str(),
            "agent_override": agent_override.map(|a| a.as_str()),
            "claude_signals": detected.claude_signals,
            "codex_signals": detected.codex_signals,
            "agents_md_present": detected.agents_md_present
        }),
    );
    push_triage_update(
        runtime,
        &mut activity,
        &mut artifacts,
        "Agent detection complete",
        Some(format!("Detected agent: {}", chosen_agent.as_str())),
    )?;

    if !runtime.terminal.stdin_is_tty() {
        runtime
            .terminal
            .write_line("━━━ Agent Detection ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        runtime
            .terminal
            .write_line(&format!("Detected agent: {:?}", detected.detected))?;
    }

    append_run_log(
        "info",
        "triage.discovery.started",
        json!({
            "backend": chosen_agent.as_str(),
            "model": cfg.seeding.model,
            "max_turns": cfg.triage.discovery_max_turns
        }),
    );
    push_triage_update(
        runtime,
        &mut activity,
        &mut artifacts,
        "Running repository discovery assessment",
        None,
    )?;
    let discovery = run_discovery(
        runtime.process_runner.as_ref(),
        scope,
        chosen_agent,
        &cfg.seeding.model,
        cfg.triage.discovery_max_turns,
    )
    .unwrap_or_else(|err| {
        append_run_log(
            "warn",
            "triage.discovery.failed",
            json!({
                "error": err.to_string()
            }),
        );
        DiscoveryAssessment::unknown()
    });
    let discovery_used = discovery.agent_steering.grade != "unknown";
    append_run_log(
        "info",
        "triage.discovery.completed",
        json!({
            "discovery_used": discovery_used,
            "overall_readiness_grade": discovery.overall_readiness_grade,
            "overall_readiness_score": discovery.overall_readiness_score,
            "primary_gap": discovery.primary_gap,
            "agent_steering_grade": discovery.agent_steering.grade,
            "knowledge_accessible_grade": discovery.knowledge_accessible.grade,
            "mechanical_guardrails_grade": discovery.mechanical_guardrails.grade,
            "local_feedback_loop_grade": discovery.local_feedback_loop.grade,
            "coverage_signal_grade": discovery.coverage_signal.grade
        }),
    );
    push_triage_update(
        runtime,
        &mut activity,
        &mut artifacts,
        "Discovery assessment complete",
        Some(format!(
            "Readiness: {} ({})",
            discovery.overall_readiness_grade, discovery.overall_readiness_score
        )),
    )?;

    append_run_log("info", "triage.interview.started", json!({}));
    push_triage_update(
        runtime,
        &mut activity,
        &mut artifacts,
        "Collecting human-validated repository context",
        None,
    )?;
    runtime.terminal.close_ui()?;
    let interview = run_interview(
        runtime.terminal.as_ref(),
        &discovery,
        cfg.orchestrator.parallelism,
        &cfg.validation.command,
    )?;
    append_run_log(
        "info",
        "triage.interview.completed",
        json!({
            "preferred_parallelism": interview.preferred_parallelism,
            "validation_command": interview.validation_command,
            "external_docs_accessible": interview.external_docs_accessible,
            "has_additional_context": !interview.additional_context.is_empty(),
            "coverage_grade_override": interview.coverage_grade_override
        }),
    );
    push_triage_update(
        runtime,
        &mut activity,
        &mut artifacts,
        "Interview complete",
        Some(format!(
            "Validation command: {}",
            interview.validation_command
        )),
    )?;

    let agents_md = detected.agents_md_present;
    let mut profile = build_profile(crate::repo_intelligence::BuildProfileInput {
        clock: runtime.clock.as_ref(),
        working_dir: &scope.working_dir,
        repo_root,
        head_sha: current_head_sha(runtime.process_runner.as_ref(), &scope.working_dir)
            .unwrap_or_else(|_| "unknown".to_string()),
        discovery,
        discovery_used,
        primary_agent: Some(chosen_agent),
        claude_signals: detected.claude_signals,
        codex_signals: detected.codex_signals,
        validation_command: interview.validation_command,
        agents_md_present: agents_md,
    });
    profile.user_validated.additional_context = interview.additional_context;
    profile.user_validated.external_docs_accessible = interview.external_docs_accessible;
    profile.user_validated.preferred_parallelism = interview.preferred_parallelism;
    profile.user_validated.agent_steering_correction = interview.agent_steering_correction;
    profile.user_validated.external_docs_surface = interview.external_docs_surface;
    profile.user_validated.guardrails_correction = interview.guardrails_correction;
    profile.user_validated.coverage_grade_override = interview.coverage_grade_override;
    let path = resolved_profile_path;
    write_profile(runtime.file_system.as_ref(), &path, &profile)?;
    push_triage_update(
        runtime,
        &mut activity,
        &mut artifacts,
        "Persisted triage profile",
        Some(format!("Repo intelligence profile: {}", path.display())),
    )?;
    append_run_log(
        "info",
        "triage.completed",
        json!({
            "path": path.display().to_string(),
            "readiness_grade": profile.agent_readiness.readiness_grade,
            "readiness_score": profile.agent_readiness.readiness_score,
            "primary_gap": profile.agent_readiness.primary_gap,
            "primary_agent": profile.detected_agent.primary
        }),
    );
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
    append_run_log(
        "debug",
        "triage.ensure_profile.started",
        json!({
            "force_retriage": force_retriage,
            "agent_override": agent_override.map(|a| a.as_str())
        }),
    );
    match triage_needed(scope, cfg, runtime, force_retriage)? {
        TriageDecision::NotNeeded => {
            let path = profile_path(scope, cfg);
            let profile = read_profile(runtime.file_system.as_ref(), &path)?;
            append_run_log(
                "info",
                "triage.ensure_profile.loaded_existing",
                json!({
                    "path": path.display().to_string(),
                    "readiness_grade": profile.agent_readiness.readiness_grade,
                    "head_sha": profile.meta.head_sha
                }),
            );
            Ok(Some(profile))
        }
        TriageDecision::Needed => {
            if is_non_interactive(env, runtime.terminal.as_ref()).is_some() {
                let path = profile_path(scope, cfg);
                append_run_log(
                    "error",
                    "triage.ensure_profile.blocked_non_interactive",
                    json!({
                        "output_path": path.display().to_string()
                    }),
                );
                return Err(GardenerError::Cli(format!(
                    "Triage requires a human and cannot run non-interactively.\n\nNo repo intelligence profile was found at {}.\nTriage gathers context that Gardener cannot determine automatically.\n\nTo complete setup, run in a terminal:\n  gardener --triage-only\n\nThen re-run your agent or pipeline.",
                    path.display()
                )));
            }
            run_triage(runtime, scope, cfg, env, agent_override).map(Some)
        }
    }
}
