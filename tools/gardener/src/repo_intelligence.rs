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
    #[serde(default)]
    pub backlog_approval: bool,
    pub corrections_made: u32,
    pub validated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentReadiness {
    pub agent_steering_score: Option<i64>,
    pub knowledge_accessible_score: Option<i64>,
    pub mechanical_guardrails_score: Option<i64>,
    pub local_feedback_loop_score: Option<i64>,
    pub coverage_signal_score: Option<i64>,
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

pub struct BuildProfileInput<'a> {
    pub clock: &'a dyn Clock,
    pub working_dir: &'a Path,
    pub repo_root: &'a Path,
    pub head_sha: String,
    pub discovery: DiscoveryAssessment,
    pub discovery_used: bool,
    pub primary_agent: Option<AgentKind>,
    pub claude_signals: Vec<String>,
    pub codex_signals: Vec<String>,
    pub validation_command: String,
    pub agents_md_present: bool,
}

pub fn build_profile(input: BuildProfileInput<'_>) -> RepoIntelligenceProfile {
    let BuildProfileInput {
        clock,
        working_dir,
        repo_root,
        head_sha,
        discovery,
        discovery_used,
        primary_agent,
        claude_signals,
        codex_signals,
        validation_command,
        agents_md_present,
    } = input;
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
            backlog_approval: false,
            corrections_made: 0,
            validated_at: now,
        },
        agent_readiness: readiness,
    }
}

fn score_for_grade(grade: &str) -> Option<i64> {
    match grade {
        "A" => Some(18),
        "B" => Some(14),
        "C" => Some(9),
        "D" => Some(5),
        "F" => Some(0),
        "unknown" => None,
        _ => None,
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
        .filter_map(|(name, v)| score_for_grade(&v.grade).map(|score| (*name, score)))
        .collect();
    let total: i64 = scores.iter().map(|(_, score)| *score).sum();
    scores.sort_by_key(|(name, score)| (*score, *name));
    let primary_gap = scores
        .first()
        .map(|(name, _)| (*name).to_string())
        .unwrap_or_else(|| "agent_steering".to_string());

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

#[cfg(test)]
mod tests {
    use super::{derive_agent_readiness, BuildProfileInput, DiscoveryAssessment};
    use crate::triage_discovery::DimensionAssessment;
    use crate::types::AgentKind;
    use crate::runtime::FakeClock;
    use std::path::PathBuf;

    fn scored_discovery(
        agent_steering: &str,
        knowledge_accessible: &str,
        mechanical: &str,
        local_feedback_loop: &str,
        coverage_signal: &str,
    ) -> DiscoveryAssessment {
        DiscoveryAssessment {
            agent_steering: DimensionAssessment {
                grade: agent_steering.to_string(),
                summary: "test".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            knowledge_accessible: DimensionAssessment {
                grade: knowledge_accessible.to_string(),
                summary: "test".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            mechanical_guardrails: DimensionAssessment {
                grade: mechanical.to_string(),
                summary: "test".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            local_feedback_loop: DimensionAssessment {
                grade: local_feedback_loop.to_string(),
                summary: "test".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            coverage_signal: DimensionAssessment {
                grade: coverage_signal.to_string(),
                summary: "test".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            overall_readiness_score: 0,
            overall_readiness_grade: "F".to_string(),
            primary_gap: "agent_steering".to_string(),
            notable_findings: String::new(),
            scope_notes: String::new(),
        }
    }

    #[test]
    fn derive_agent_readiness_marks_unknown_as_unscored() {
        let discovery = scored_discovery("unknown", "A", "F", "B", "C");
        let readiness = derive_agent_readiness(&discovery);
        assert_eq!(readiness.agent_steering_score, None);
        assert_eq!(readiness.knowledge_accessible_score, Some(18));
        assert_eq!(readiness.mechanical_guardrails_score, Some(0));
        assert_eq!(readiness.local_feedback_loop_score, Some(14));
        assert_eq!(readiness.coverage_signal_score, Some(9));
        assert_eq!(readiness.readiness_score, 41);
        assert_eq!(readiness.readiness_grade, "D");
        assert_eq!(readiness.primary_gap, "mechanical_guardrails");
    }

    #[test]
    fn derive_agent_readiness_without_known_grades_stays_unknown_gap() {
        let discovery = scored_discovery("unknown", "unknown", "unknown", "unknown", "unknown");
        let readiness = derive_agent_readiness(&discovery);
        assert_eq!(readiness.agent_steering_score, None);
        assert_eq!(readiness.knowledge_accessible_score, None);
        assert_eq!(readiness.mechanical_guardrails_score, None);
        assert_eq!(readiness.local_feedback_loop_score, None);
        assert_eq!(readiness.coverage_signal_score, None);
        assert_eq!(readiness.readiness_score, 0);
        assert_eq!(readiness.readiness_grade, "F");
        assert_eq!(readiness.primary_gap, "agent_steering");
    }

    #[test]
    fn build_profile_uses_unknown_discovery_without_scoring() {
        let clock = FakeClock::new(std::time::UNIX_EPOCH);
        let repo_root = PathBuf::from("/repo");
        let working_dir = PathBuf::from("/repo");
        let discovery = DiscoveryAssessment::unknown();
        let profile = super::build_profile(BuildProfileInput {
            clock: &clock,
            working_dir: &working_dir,
            repo_root: &repo_root,
            head_sha: "head".to_string(),
            discovery,
            discovery_used: false,
            primary_agent: Some(AgentKind::Codex),
            claude_signals: Vec::new(),
            codex_signals: Vec::new(),
            validation_command: "npm run validate".to_string(),
            agents_md_present: false,
        });
        assert!(!profile.meta.discovery_used);
        assert_eq!(profile.agent_readiness.agent_steering_score, None);
        assert_eq!(profile.agent_readiness.knowledge_accessible_score, None);
        assert_eq!(profile.agent_readiness.coverage_signal_score, None);
        assert_eq!(profile.agent_readiness.readiness_score, 0);
        assert_eq!(profile.agent_readiness.primary_gap, "agent_steering");
    }
}
