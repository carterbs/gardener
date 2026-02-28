use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::runtime::Terminal;
use crate::triage_discovery::DiscoveryAssessment;
use crate::tui::run_repo_health_wizard;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterviewResult {
    pub preferred_parallelism: Option<u32>,
    pub validation_command: String,
    pub additional_context: String,
    pub external_docs_accessible: bool,
    pub agent_steering_correction: String,
    pub external_docs_surface: String,
    pub guardrails_correction: String,
    pub coverage_grade_override: String,
}

pub fn run_interview(
    terminal: &dyn Terminal,
    discovery: &DiscoveryAssessment,
    default_parallelism: u32,
    default_validation_command: &str,
) -> Result<InterviewResult, GardenerError> {
    let is_tty = terminal.stdin_is_tty();
    append_run_log(
        "info",
        "triage.interview.mode",
        json!({
            "interactive": is_tty,
            "default_parallelism": default_parallelism,
            "default_validation_command": default_validation_command,
            "agent_steering_grade": discovery.agent_steering.grade,
            "knowledge_accessible_grade": discovery.knowledge_accessible.grade,
            "mechanical_guardrails_grade": discovery.mechanical_guardrails.grade,
            "coverage_signal_grade": discovery.coverage_signal.grade
        }),
    );

    if !is_tty {
        terminal
            .write_line("━━━ Agent Steering ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        terminal.write_line(&format!(
            "Agent assessment: {} — {}",
            discovery.agent_steering.grade, discovery.agent_steering.summary
        ))?;
        terminal
            .write_line("━━━ Knowledge Accessibility ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        terminal.write_line(&format!(
            "Agent assessment: {} — {}",
            discovery.knowledge_accessible.grade, discovery.knowledge_accessible.summary
        ))?;
        terminal
            .write_line("━━━ Mechanical Guardrails ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        terminal.write_line(&format!(
            "Agent assessment: {} — {}",
            discovery.mechanical_guardrails.grade, discovery.mechanical_guardrails.summary
        ))?;
        terminal
            .write_line("━━━ Local Feedback Loop ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        terminal.write_line(&format!(
            "Detected validation command: {default_validation_command}"
        ))?;
        terminal
            .write_line("━━━ Coverage Signal ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        terminal.write_line(&format!(
            "Agent assessment: {} — {}",
            discovery.coverage_signal.grade, discovery.coverage_signal.summary
        ))?;
        terminal
            .write_line("━━━ Anything Else? ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    }

    let mut preferred_parallelism = Some(default_parallelism);
    let mut validation_command = default_validation_command.to_string();
    let mut additional_context = String::new();
    let mut external_docs_accessible = true;
    let agent_steering_correction = format!(
        "{}: {}",
        discovery.agent_steering.grade, discovery.agent_steering.summary
    );
    let external_docs_surface = format!(
        "{}: {}",
        discovery.knowledge_accessible.grade, discovery.knowledge_accessible.summary
    );
    let guardrails_correction = format!(
        "{}: {}",
        discovery.mechanical_guardrails.grade, discovery.mechanical_guardrails.summary
    );
    let coverage_grade_override = discovery.coverage_signal.grade.clone();
    if is_tty {
        match run_repo_health_wizard(default_validation_command) {
            Ok(answers) => {
                append_run_log(
                    "info",
                    "triage.interview.wizard_completed",
                    json!({
                        "preferred_parallelism": answers.preferred_parallelism,
                        "validation_command": answers.validation_command,
                        "external_docs_accessible": answers.external_docs_accessible,
                        "has_additional_context": !answers.additional_context.is_empty()
                    }),
                );
                preferred_parallelism = Some(answers.preferred_parallelism);
                validation_command = answers.validation_command;
                external_docs_accessible = answers.external_docs_accessible;
                additional_context = answers.additional_context;
            }
            Err(err) => {
                append_run_log(
                    "warn",
                    "triage.interview.wizard_unavailable",
                    json!({
                        "error": err.to_string()
                    }),
                );
                terminal.write_line(
                    "TUI setup unavailable; using defaults for repo-health bootstrap.",
                )?;
            }
        }
    }

    append_run_log(
        "debug",
        "triage.interview.result",
        json!({
            "preferred_parallelism": preferred_parallelism,
            "validation_command": validation_command,
            "external_docs_accessible": external_docs_accessible,
            "has_additional_context": !additional_context.is_empty(),
            "coverage_grade_override": coverage_grade_override
        }),
    );

    Ok(InterviewResult {
        preferred_parallelism,
        validation_command,
        additional_context,
        external_docs_accessible,
        agent_steering_correction,
        external_docs_surface,
        guardrails_correction,
        coverage_grade_override,
    })
}

#[cfg(test)]
mod tests {
    use super::run_interview;
    use crate::runtime::FakeTerminal;
    use crate::triage_discovery::DimensionAssessment;
    use crate::triage_discovery::DiscoveryAssessment;

    fn discovery(grade: &str) -> DiscoveryAssessment {
        DiscoveryAssessment {
            agent_steering: DimensionAssessment {
                grade: grade.to_string(),
                summary: "agent".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            knowledge_accessible: DimensionAssessment {
                grade: grade.to_string(),
                summary: "knowledge".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            mechanical_guardrails: DimensionAssessment {
                grade: grade.to_string(),
                summary: "guardrails".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            local_feedback_loop: DimensionAssessment {
                grade: grade.to_string(),
                summary: "feedback".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            coverage_signal: DimensionAssessment {
                grade: grade.to_string(),
                summary: "coverage".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            overall_readiness_score: 0,
            overall_readiness_grade: grade.to_string(),
            primary_gap: "agent_steering".to_string(),
            notable_findings: "none".to_string(),
            scope_notes: String::new(),
        }
    }

    #[test]
    fn non_tty_path_formats_read_only_sections() {
        let terminal = FakeTerminal::new(false);
        let result = run_interview(
            &terminal,
            &discovery("B"),
            4,
            "cargo test --all-targets",
        )
        .expect("interview");

        assert_eq!(result.preferred_parallelism, Some(4));
        assert_eq!(result.validation_command, "cargo test --all-targets");
        assert_eq!(result.additional_context, String::new());
        assert_eq!(result.external_docs_accessible, true);
        assert_eq!(result.agent_steering_correction, "B: agent");
    }
}
