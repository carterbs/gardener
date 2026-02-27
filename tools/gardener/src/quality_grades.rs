use crate::quality_domain_catalog::discover_domains;
use crate::quality_evidence::collect_evidence;
use crate::quality_scoring::score_domains;
use crate::repo_intelligence::RepoIntelligenceProfile;

pub fn render_quality_grade_document(
    profile_path: &str,
    profile: &RepoIntelligenceProfile,
) -> String {
    let domains = discover_domains();
    let evidence = collect_evidence(&domains);
    let has_coverage_gates = profile.agent_readiness.coverage_signal_score > 0;
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
    out.push_str("## Agent Readiness\n");
    out.push_str("| Dimension | Score |\n| --- | --- |\n");
    out.push_str(&format!(
        "| agent_steering | {} |\n| knowledge_accessible | {} |\n| mechanical_guardrails | {} |\n| local_feedback_loop | {} |\n| coverage_signal | {} |\n\n",
        profile.agent_readiness.agent_steering_score,
        profile.agent_readiness.knowledge_accessible_score,
        profile.agent_readiness.mechanical_guardrails_score,
        profile.agent_readiness.local_feedback_loop_score,
        profile.agent_readiness.coverage_signal_score,
    ));
    out.push_str("## Coverage Detail\n");
    out.push_str("| Domain | Score | Grade |\n| --- | --- | --- |\n");
    for score in scores {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            score.domain, score.score, score.grade
        ));
    }
    out.push('\n');
    out
}
