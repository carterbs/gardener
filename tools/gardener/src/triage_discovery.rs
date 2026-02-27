use crate::errors::GardenerError;
use crate::output_envelope::parse_last_envelope;
use crate::runtime::{ProcessRequest, ProcessRunner};
use crate::types::{AgentKind, RuntimeScope, WorkerState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DimensionAssessment {
    pub grade: String,
    pub summary: String,
    pub issues: Vec<String>,
    pub strengths: Vec<String>,
}

impl DimensionAssessment {
    fn unknown() -> Self {
        Self {
            grade: "unknown".to_string(),
            summary: "discovery unavailable".to_string(),
            issues: Vec::new(),
            strengths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryAssessment {
    pub agent_steering: DimensionAssessment,
    pub knowledge_accessible: DimensionAssessment,
    pub mechanical_guardrails: DimensionAssessment,
    pub local_feedback_loop: DimensionAssessment,
    pub coverage_signal: DimensionAssessment,
    pub overall_readiness_score: i64,
    pub overall_readiness_grade: String,
    pub primary_gap: String,
    pub notable_findings: String,
    pub scope_notes: String,
}

impl DiscoveryAssessment {
    pub fn unknown() -> Self {
        Self {
            agent_steering: DimensionAssessment::unknown(),
            knowledge_accessible: DimensionAssessment::unknown(),
            mechanical_guardrails: DimensionAssessment::unknown(),
            local_feedback_loop: DimensionAssessment::unknown(),
            coverage_signal: DimensionAssessment::unknown(),
            overall_readiness_score: 10,
            overall_readiness_grade: "F".to_string(),
            primary_gap: "agent_steering".to_string(),
            notable_findings: "discovery unavailable".to_string(),
            scope_notes: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiscoveryEnvelope {
    gardener_output: DiscoveryAssessment,
}

pub fn build_discovery_prompt(scope: &RuntimeScope) -> String {
    let mut prompt = format!(
        "WORKING DIRECTORY: {}\nREPOSITORY ROOT: {}\n",
        scope.working_dir.display(),
        scope
            .repo_root
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| scope.working_dir.display().to_string())
    );
    if scope.repo_root.as_ref() != Some(&scope.working_dir) {
        prompt.push_str("Note: scoped run; include root-level signals in scope_notes.\n");
    }
    prompt.push_str("Return an output envelope with gardener_output.");
    prompt
}

pub fn run_discovery(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    backend: AgentKind,
    model: &str,
    max_turns: u32,
) -> Result<DiscoveryAssessment, GardenerError> {
    let prompt = build_discovery_prompt(scope);
    let (program, args) = match backend {
        AgentKind::Codex => (
            "codex".to_string(),
            vec![
                "exec".to_string(),
                "--json".to_string(),
                "--model".to_string(),
                model.to_string(),
                "--max-turns".to_string(),
                max_turns.to_string(),
                prompt,
            ],
        ),
        AgentKind::Claude => (
            "claude".to_string(),
            vec![
                "-p".to_string(),
                prompt,
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--model".to_string(),
                model.to_string(),
            ],
        ),
    };

    let output = process_runner.run(ProcessRequest {
        program,
        args,
        cwd: Some(scope.working_dir.clone()),
    })?;

    if output.exit_code != 0 {
        return Err(GardenerError::Process(output.stderr));
    }

    let envelope = parse_last_envelope(&output.stdout, WorkerState::Seeding)?;
    let parsed: DiscoveryEnvelope = serde_json::from_value(envelope.payload)
        .map_err(|e| GardenerError::OutputEnvelope(e.to_string()))?;
    Ok(parsed.gardener_output)
}
