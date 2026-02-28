use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::runtime::{Clock, FileSystem, ProcessRequest, ProcessRunner};
use crate::triage_discovery::DiscoveryAssessment;
use crate::types::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoIntelligenceProfile {
    pub meta: RepoMeta,
    pub detected_agent: DetectedAgentProfile,
    pub discovery: DiscoveryAssessment,
    pub user_validated: UserValidated,
    pub agent_readiness: AgentReadiness,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoMeta {
    pub schema_version: u32,
    pub created_at: String,
    pub head_sha: String,
    pub working_dir: String,
    pub repo_root: String,
    pub discovery_used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetectedAgentProfile {
    pub primary: String,
    pub claude_signals: Vec<String>,
    pub codex_signals: Vec<String>,
    pub agents_md_present: bool,
    pub user_confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserValidated {
    pub agent_steering_correction: String,
    pub external_docs_surface: String,
    pub external_docs_accessible: bool,
    pub guardrails_correction: String,
    pub validation_command: String,
    pub coverage_grade_override: String,
    pub additional_context: String,
    #[serde(default)]
    pub preferred_parallelism: Option<u32>,
    pub corrections_made: u32,
    pub validated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentReadiness {
    pub agent_steering_score: i64,
    pub knowledge_accessible_score: i64,
    pub mechanical_guardrails_score: i64,
    pub local_feedback_loop_score: i64,
    pub coverage_signal_score: i64,
    pub readiness_score: i64,
    pub readiness_grade: String,
    pub primary_gap: String,
}

pub fn write_profile(
    fs: &dyn FileSystem,
    path: &Path,
    profile: &RepoIntelligenceProfile,
) -> Result<(), GardenerError> {
    if let Some(parent) = path.parent() {
        fs.create_dir_all(parent)?;
    }
    let toml =
        toml::to_string_pretty(profile).map_err(|e| GardenerError::ConfigParse(e.to_string()))?;
    append_run_log(
        "info",
        "repo_intelligence.profile.written",
        json!({
            "path": path.display().to_string(),
            "schema_version": profile.meta.schema_version,
            "head_sha": profile.meta.head_sha,
            "primary_agent": profile.detected_agent.primary,
            "readiness_grade": profile.agent_readiness.readiness_grade,
            "readiness_score": profile.agent_readiness.readiness_score
        }),
    );
    fs.write_string(path, &toml)
}

pub fn read_profile(
    fs: &dyn FileSystem,
    path: &Path,
) -> Result<RepoIntelligenceProfile, GardenerError> {
    append_run_log(
        "debug",
        "repo_intelligence.profile.reading",
        json!({
            "path": path.display().to_string()
        }),
    );
    let raw = fs.read_to_string(path)?;
    let profile: RepoIntelligenceProfile =
        toml::from_str(&raw).map_err(|e| GardenerError::ConfigParse(e.to_string()))?;
    append_run_log(
        "debug",
        "repo_intelligence.profile.read",
        json!({
            "path": path.display().to_string(),
            "head_sha": profile.meta.head_sha,
            "schema_version": profile.meta.schema_version,
            "primary_agent": profile.detected_agent.primary,
            "readiness_grade": profile.agent_readiness.readiness_grade
        }),
    );
    Ok(profile)
}

pub fn current_head_sha(
    process_runner: &dyn ProcessRunner,
    cwd: &Path,
) -> Result<String, GardenerError> {
    let out = process_runner.run(ProcessRequest {
        program: "git".to_string(),
        args: vec!["rev-parse".to_string(), "HEAD".to_string()],
        cwd: Some(cwd.to_path_buf()),
    })?;
    if out.exit_code != 0 {
        append_run_log(
            "warn",
            "repo_intelligence.git.head_sha_failed",
            json!({
                "cwd": cwd.display().to_string(),
                "exit_code": out.exit_code,
                "stderr": out.stderr
            }),
        );
        return Err(GardenerError::Process(out.stderr));
    }
    let sha = out.stdout.trim().to_string();
    append_run_log(
        "debug",
        "repo_intelligence.git.head_sha",
        json!({
            "cwd": cwd.display().to_string(),
            "sha": sha
        }),
    );
    Ok(sha)
}

pub fn commits_since_profile_head(
    process_runner: &dyn ProcessRunner,
    cwd: &Path,
    profile_head: &str,
) -> Result<u64, GardenerError> {
    let out = process_runner.run(ProcessRequest {
        program: "git".to_string(),
        args: vec![
            "rev-list".to_string(),
            "--count".to_string(),
            format!("{profile_head}..HEAD"),
        ],
        cwd: Some(cwd.to_path_buf()),
    })?;
    if out.exit_code != 0 {
        append_run_log(
            "warn",
            "repo_intelligence.git.commits_since_failed",
            json!({
                "cwd": cwd.display().to_string(),
                "profile_head": profile_head,
                "exit_code": out.exit_code
            }),
        );
        return Ok(0);
    }
    let count = out.stdout.trim().parse::<u64>().unwrap_or(0);
    append_run_log(
        "debug",
        "repo_intelligence.git.commits_since",
        json!({
            "cwd": cwd.display().to_string(),
            "profile_head": profile_head,
            "commits_since": count
        }),
    );
    Ok(count)
}

pub fn build_profile(
    clock: &dyn Clock,
    working_dir: &Path,
    repo_root: &Path,
    head_sha: String,
    discovery: DiscoveryAssessment,
    discovery_used: bool,
    primary_agent: Option<AgentKind>,
    claude_signals: Vec<String>,
    codex_signals: Vec<String>,
    validation_command: String,
    agents_md_present: bool,
) -> RepoIntelligenceProfile {
    append_run_log(
        "info",
        "repo_intelligence.build_profile.started",
        json!({
            "working_dir": working_dir.display().to_string(),
            "repo_root": repo_root.display().to_string(),
            "head_sha": head_sha,
            "discovery_used": discovery_used,
            "primary_agent": primary_agent.map(|a| a.as_str()),
            "agents_md_present": agents_md_present,
            "validation_command": validation_command,
            "claude_signals_count": claude_signals.len(),
            "codex_signals_count": codex_signals.len()
        }),
    );

    let now_secs = clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let now = format!("{now_secs}");

    let mut effective_discovery = discovery;
    if !discovery_used {
        append_run_log(
            "debug",
            "repo_intelligence.build_profile.discovery_skipped",
            json!({ "reason": "discovery_used=false, substituting unknown assessment" }),
        );
        effective_discovery = DiscoveryAssessment::unknown();
    }

    let readiness = derive_agent_readiness(&effective_discovery);
    append_run_log(
        "info",
        "repo_intelligence.build_profile.readiness",
        json!({
            "readiness_grade": readiness.readiness_grade,
            "readiness_score": readiness.readiness_score,
            "primary_gap": readiness.primary_gap,
            "agent_steering_score": readiness.agent_steering_score,
            "knowledge_accessible_score": readiness.knowledge_accessible_score,
            "mechanical_guardrails_score": readiness.mechanical_guardrails_score,
            "local_feedback_loop_score": readiness.local_feedback_loop_score,
            "coverage_signal_score": readiness.coverage_signal_score
        }),
    );
    RepoIntelligenceProfile {
        meta: RepoMeta {
            schema_version: 1,
            created_at: now.clone(),
            head_sha,
            working_dir: working_dir.display().to_string(),
            repo_root: repo_root.display().to_string(),
            discovery_used,
        },
        detected_agent: DetectedAgentProfile {
            primary: primary_agent
                .map(|v| v.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            claude_signals,
            codex_signals,
            agents_md_present,
            user_confirmed: true,
        },
        discovery: effective_discovery,
        user_validated: UserValidated {
            agent_steering_correction: String::new(),
            external_docs_surface: String::new(),
            external_docs_accessible: true,
            guardrails_correction: String::new(),
            validation_command,
            coverage_grade_override: String::new(),
            additional_context: String::new(),
            preferred_parallelism: None,
            corrections_made: 0,
            validated_at: now,
        },
        agent_readiness: readiness,
    }
}

fn score_for_grade(grade: &str) -> i64 {
    match grade {
        "A" => 18,
        "B" => 14,
        "C" => 9,
        "D" => 5,
        "F" => 0,
        _ => 2,
    }
}

pub fn derive_agent_readiness(discovery: &DiscoveryAssessment) -> AgentReadiness {
    let dims = [
        ("agent_steering", &discovery.agent_steering),
        ("knowledge_accessible", &discovery.knowledge_accessible),
        ("mechanical_guardrails", &discovery.mechanical_guardrails),
        ("local_feedback_loop", &discovery.local_feedback_loop),
        ("coverage_signal", &discovery.coverage_signal),
    ];
    let mut scores: Vec<(&str, i64)> = dims
        .iter()
        .map(|(name, v)| (*name, score_for_grade(&v.grade)))
        .collect();
    let total: i64 = scores.iter().map(|(_, score)| *score).sum();
    scores.sort_by_key(|(name, score)| (*score, *name));
    let primary_gap = scores
        .first()
        .map(|(name, _)| (*name).to_string())
        .unwrap_or_default();

    AgentReadiness {
        agent_steering_score: score_for_grade(&discovery.agent_steering.grade),
        knowledge_accessible_score: score_for_grade(&discovery.knowledge_accessible.grade),
        mechanical_guardrails_score: score_for_grade(&discovery.mechanical_guardrails.grade),
        local_feedback_loop_score: score_for_grade(&discovery.local_feedback_loop.grade),
        coverage_signal_score: score_for_grade(&discovery.coverage_signal.grade),
        readiness_score: total,
        readiness_grade: readiness_grade(total).to_string(),
        primary_gap,
    }
}

fn readiness_grade(score: i64) -> &'static str {
    match score {
        90..=100 => "A",
        75..=89 => "B",
        60..=74 => "C",
        40..=59 => "D",
        _ => "F",
    }
}
