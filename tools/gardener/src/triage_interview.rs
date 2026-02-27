use crate::errors::GardenerError;
use crate::runtime::Terminal;
use crate::triage_discovery::DiscoveryAssessment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterviewResult {
    pub validation_command: String,
    pub additional_context: String,
    pub external_docs_accessible: bool,
}

pub fn run_interview(
    terminal: &dyn Terminal,
    discovery: &DiscoveryAssessment,
    default_validation_command: &str,
) -> Result<InterviewResult, GardenerError> {
    terminal.write_line("━━━ Agent Steering ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    terminal.write_line(&format!(
        "Agent assessment: {} — {}",
        discovery.agent_steering.grade, discovery.agent_steering.summary
    ))?;
    terminal.write_line("━━━ Knowledge Accessibility ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    terminal.write_line(&format!(
        "Agent assessment: {} — {}",
        discovery.knowledge_accessible.grade, discovery.knowledge_accessible.summary
    ))?;
    terminal.write_line("━━━ Mechanical Guardrails ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    terminal.write_line(&format!(
        "Agent assessment: {} — {}",
        discovery.mechanical_guardrails.grade, discovery.mechanical_guardrails.summary
    ))?;
    terminal.write_line("━━━ Local Feedback Loop ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    terminal.write_line(&format!(
        "Detected validation command: {default_validation_command}"
    ))?;
    terminal.write_line("━━━ Coverage Signal ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    terminal.write_line(&format!(
        "Agent assessment: {} — {}",
        discovery.coverage_signal.grade, discovery.coverage_signal.summary
    ))?;
    terminal.write_line("━━━ Anything Else? ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;

    Ok(InterviewResult {
        validation_command: default_validation_command.to_string(),
        additional_context: String::new(),
        external_docs_accessible: true,
    })
}
