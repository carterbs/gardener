use crate::errors::GardenerError;
use crate::runtime::{Clock, FileSystem, ProcessRequest, ProcessRunner};
use crate::triage_discovery::{DimensionAssessment, DiscoveryAssessment};
use crate::types::AgentKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
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
    fs.write_string(path, &toml)
}

pub fn read_profile(
    fs: &dyn FileSystem,
    path: &Path,
) -> Result<RepoIntelligenceProfile, GardenerError> {
    let raw = fs.read_to_string(path)?;
    toml::from_str(&raw).map_err(|e| GardenerError::ConfigParse(e.to_string()))
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
        return Err(GardenerError::Process(out.stderr));
    }
    Ok(out.stdout.trim().to_string())
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
        return Ok(0);
    }
    Ok(out.stdout.trim().parse::<u64>().unwrap_or(0))
}

pub fn is_stale(
    profile: &RepoIntelligenceProfile,
    process_runner: &dyn ProcessRunner,
    cwd: &Path,
    stale_after_commits: u64,
) -> bool {
    if let Ok(current) = current_head_sha(process_runner, cwd) {
        if current == profile.meta.head_sha {
            return false;
        }
        if let Ok(diff) = commits_since_profile_head(process_runner, cwd, &profile.meta.head_sha) {
            return diff > stale_after_commits;
        }
        return true;
    }
    false
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
) -> RepoIntelligenceProfile {
    let now_secs = clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let now = format!("{now_secs}");

    let mut effective_discovery = discovery.clone();
    if !discovery_used {
        effective_discovery = DiscoveryAssessment::unknown();
    }

    let readiness = derive_agent_readiness(&effective_discovery);
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
            agents_md_present: false,
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

#[allow(dead_code)]
pub fn profile_path_from_config(path: &str, working_dir: &PathBuf) -> PathBuf {
    let profile = PathBuf::from(path);
    if profile.is_absolute() {
        profile
    } else {
        working_dir.join(profile)
    }
}

#[allow(dead_code)]
fn _is_unknown(dim: &DimensionAssessment) -> bool {
    dim.grade == "unknown"
}
