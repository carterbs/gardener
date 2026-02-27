use crate::errors::GardenerError;
use crate::runtime::Terminal;
use crate::tui::run_repo_health_wizard;
use crate::triage_discovery::DiscoveryAssessment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterviewResult {
    pub preferred_parallelism: Option<u32>,
    pub validation_command: String,
    pub additional_context: String,
    pub external_docs_accessible: bool,
}

pub fn run_interview(
    terminal: &dyn Terminal,
    discovery: &DiscoveryAssessment,
    default_parallelism: u32,
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

    let mut preferred_parallelism = Some(default_parallelism);
    let mut validation_command = default_validation_command.to_string();
    let mut additional_context = String::new();
    let mut external_docs_accessible = true;
    if terminal.stdin_is_tty() {
        terminal.write_line("━━━ Repo Health Setup Wizard ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        if needs_repo_health_wizard(discovery) {
            terminal.write_line(
                "Discovery could not confirm repo-health docs and guardrails automatically.",
            )?;
        }
        terminal.write_line("Let's capture a few basics so Gardener can bootstrap safely.")?;
        match run_repo_health_wizard(default_validation_command) {
            Ok(answers) => {
                preferred_parallelism = Some(answers.preferred_parallelism);
                validation_command = answers.validation_command;
                external_docs_accessible = answers.external_docs_accessible;
                additional_context = answers.additional_context;
            }
            Err(_) => {
                terminal.write_line(
                    "TUI setup unavailable; using defaults for repo-health bootstrap.",
                )?;
            }
        }
    }

    Ok(InterviewResult {
        preferred_parallelism,
        validation_command,
        additional_context,
        external_docs_accessible,
    })
}

fn needs_repo_health_wizard(discovery: &DiscoveryAssessment) -> bool {
    [
        &discovery.agent_steering.grade,
        &discovery.knowledge_accessible.grade,
        &discovery.mechanical_guardrails.grade,
        &discovery.coverage_signal.grade,
    ]
    .iter()
    .any(|grade| grade.as_str() == "unknown")
}
