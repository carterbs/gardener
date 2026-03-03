use crate::logging::append_run_log;
use crate::quality_domain_catalog::discover_domains;
use crate::quality_evidence::collect_evidence;
use crate::quality_scoring::score_domains;
use crate::repo_intelligence::RepoIntelligenceProfile;
use serde_json::json;
use std::path::Path;

pub fn render_quality_grade_document(
    profile_path: &str,
    profile: &RepoIntelligenceProfile,
    repo_root: &Path,
) -> String {
    append_run_log(
        "info",
        "quality.grades.render.started",
        json!({
            "profile_path": profile_path,
            "repo_root": repo_root.display().to_string(),
            "readiness_score": profile.agent_readiness.readiness_score,
            "readiness_grade": profile.agent_readiness.readiness_grade
        }),
    );

    let domains = discover_domains(repo_root);
    append_run_log(
        "debug",
        "quality.grades.domains.discovered",
        json!({
            "domain_count": domains.len()
        }),
    );

    let evidence = collect_evidence(&domains, repo_root);
    append_run_log(
        "debug",
        "quality.grades.evidence.collected",
        json!({
            "evidence_count": evidence.len()
        }),
    );

    let has_coverage_gates = matches!(
        profile.agent_readiness.coverage_signal_score,
        Some(score) if score > 0
    );
    let scores = score_domains(&evidence, has_coverage_gates);

    let mut out = String::new();
    out.push_str(&format!(
        "# Quality Grades\n\nReadiness: {}/100 ({})\n\n",
        profile.agent_readiness.readiness_score, profile.agent_readiness.readiness_grade
    ));
    out.push_str("## Triage Baseline\n");
    out.push_str(&format!("- profile_path: {profile_path}\n"));
    out.push_str(&format!(
        "- readiness_score: {}\n- readiness_grade: {}\n- primary_gap: {}\n\n",
        profile.agent_readiness.readiness_score,
        profile.agent_readiness.readiness_grade,
        profile.agent_readiness.primary_gap
    ));
    if !profile.meta.discovery_used {
        out.push_str("## Discovery Status\n");
        out.push_str(
            "Discovery was unavailable during triage; scores in Agent Readiness are marked as `unknown`.\n\n",
        );
    }
    out.push_str("## Agent Readiness\n");
    out.push_str("| Dimension | Score |\n| --- | --- |\n");
    out.push_str(&format!(
        "| agent_steering | {} |\n| knowledge_accessible | {} |\n| mechanical_guardrails | {} |\n| local_feedback_loop | {} |\n| coverage_signal | {} |\n\n",
        format_dimension_score(
            &profile.discovery.agent_steering.grade,
            profile.agent_readiness.agent_steering_score
        ),
        format_dimension_score(
            &profile.discovery.knowledge_accessible.grade,
            profile.agent_readiness.knowledge_accessible_score
        ),
        format_dimension_score(
            &profile.discovery.mechanical_guardrails.grade,
            profile.agent_readiness.mechanical_guardrails_score
        ),
        format_dimension_score(
            &profile.discovery.local_feedback_loop.grade,
            profile.agent_readiness.local_feedback_loop_score
        ),
        format_dimension_score(
            &profile.discovery.coverage_signal.grade,
            profile.agent_readiness.coverage_signal_score
        )
    ));
    out.push_str("## Coverage Detail\n");

    out.push_str("| Domain | Score | Grade |\n| --- | --- | --- |\n");
    for score in &scores {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            score.domain, score.score, score.grade
        ));
    }
    out.push('\n');

    append_run_log(
        "info",
        "quality.grades.render.completed",
        json!({
            "profile_path": profile_path,
            "scored_domains": scores.len(),
            "output_bytes": out.len()
        }),
    );

    out
}

fn format_dimension_score(discovery_grade: &str, score: Option<i64>) -> String {
    if discovery_grade == "unknown" {
        "unknown".to_string()
    } else if let Some(score) = score {
        score.to_string()
    } else {
        "unknown".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::render_quality_grade_document;
    use crate::repo_intelligence::{
        AgentReadiness, DetectedAgentProfile, RepoIntelligenceProfile, RepoMeta, UserValidated,
    };
    use crate::triage_discovery::{DimensionAssessment, DiscoveryAssessment};
    use std::path::Path;

    fn sample_profile(discovery_used: bool, unknown_grades: bool) -> RepoIntelligenceProfile {
        let grade = if unknown_grades { "unknown" } else { "A" };
        RepoIntelligenceProfile {
            meta: RepoMeta {
                schema_version: 1,
                created_at: "0".to_string(),
                head_sha: "abcd".to_string(),
                working_dir: "/repo".to_string(),
                repo_root: "/repo".to_string(),
                discovery_used,
            },
            detected_agent: DetectedAgentProfile {
                primary: "codex".to_string(),
                claude_signals: Vec::new(),
                codex_signals: Vec::new(),
                agents_md_present: false,
                user_confirmed: true,
            },
            discovery: DiscoveryAssessment {
                agent_steering: DimensionAssessment {
                    grade: grade.to_string(),
                    summary: "summary".to_string(),
                    issues: Vec::new(),
                    strengths: Vec::new(),
                },
                knowledge_accessible: DimensionAssessment {
                    grade: grade.to_string(),
                    summary: "summary".to_string(),
                    issues: Vec::new(),
                    strengths: Vec::new(),
                },
                mechanical_guardrails: DimensionAssessment {
                    grade: grade.to_string(),
                    summary: "summary".to_string(),
                    issues: Vec::new(),
                    strengths: Vec::new(),
                },
                local_feedback_loop: DimensionAssessment {
                    grade: grade.to_string(),
                    summary: "summary".to_string(),
                    issues: Vec::new(),
                    strengths: Vec::new(),
                },
                coverage_signal: DimensionAssessment {
                    grade: grade.to_string(),
                    summary: "summary".to_string(),
                    issues: Vec::new(),
                    strengths: Vec::new(),
                },
                overall_readiness_score: 10,
                overall_readiness_grade: if unknown_grades { "F" } else { "A" }.to_string(),
                primary_gap: "agent_steering".to_string(),
                notable_findings: String::new(),
                scope_notes: String::new(),
            },
            user_validated: UserValidated {
                agent_steering_correction: String::new(),
                external_docs_surface: String::new(),
                external_docs_accessible: true,
                guardrails_correction: String::new(),
                validation_command: "npm run validate".to_string(),
                coverage_grade_override: String::new(),
                additional_context: String::new(),
                preferred_parallelism: None,
                backlog_approval: false,
                corrections_made: 0,
                validated_at: "0".to_string(),
            },
            agent_readiness: AgentReadiness {
                agent_steering_score: Some(18),
                knowledge_accessible_score: Some(18),
                mechanical_guardrails_score: Some(18),
                local_feedback_loop_score: Some(18),
                coverage_signal_score: Some(18),
                readiness_score: 90,
                readiness_grade: "A".to_string(),
                primary_gap: "agent_steering".to_string(),
            },
        }
    }

    #[test]
    fn renders_agent_readiness_with_unknown_when_discovery_unavailable() {
        let profile = sample_profile(false, true);
        let output = render_quality_grade_document("profile.toml", &profile, Path::new("/repo"));
        assert!(output.contains("Discovery was unavailable"));
        assert!(output.contains("| agent_steering | unknown |"));
        assert!(output.contains("| knowledge_accessible | unknown |"));
    }

    #[test]
    fn does_not_annotate_quality_doc_when_discovery_available() {
        let profile = sample_profile(true, false);
        let output = render_quality_grade_document("profile.toml", &profile, Path::new("/repo"));
        assert!(!output.contains("Discovery was unavailable"));
        assert!(output.contains("| agent_steering | 18 |"));
    }
}
