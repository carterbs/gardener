use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::output_envelope::parse_last_envelope;
use crate::runtime::{ProcessRequest, ProcessRunner};
use crate::types::{AgentKind, RuntimeScope, WorkerState};
use serde::{Deserialize, Serialize};
use serde_json::json;

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
    append_run_log(
        "info",
        "triage.discovery.run.started",
        json!({
            "backend": backend.as_str(),
            "model": model,
            "max_turns": max_turns,
            "working_dir": scope.working_dir.display().to_string()
        }),
    );

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
        program: program.clone(),
        args,
        cwd: Some(scope.working_dir.clone()),
    })?;

    if output.exit_code != 0 {
        append_run_log(
            "error",
            "triage.discovery.run.process_failed",
            json!({
                "backend": backend.as_str(),
                "program": program,
                "exit_code": output.exit_code,
                "stderr": output.stderr
            }),
        );
        return Err(GardenerError::Process(output.stderr));
    }

    let envelope = parse_last_envelope(&output.stdout, WorkerState::Seeding)?;
    let parsed: DiscoveryEnvelope = serde_json::from_value(envelope.payload)
        .map_err(|e| GardenerError::OutputEnvelope(e.to_string()))?;
    let assessment = parsed.gardener_output;

    append_run_log(
        "info",
        "triage.discovery.run.completed",
        json!({
            "backend": backend.as_str(),
            "overall_readiness_grade": assessment.overall_readiness_grade,
            "overall_readiness_score": assessment.overall_readiness_score,
            "primary_gap": assessment.primary_gap,
            "agent_steering_grade": assessment.agent_steering.grade,
            "knowledge_accessible_grade": assessment.knowledge_accessible.grade,
            "mechanical_guardrails_grade": assessment.mechanical_guardrails.grade,
            "local_feedback_loop_grade": assessment.local_feedback_loop.grade,
            "coverage_signal_grade": assessment.coverage_signal.grade
        }),
    );

    Ok(assessment)
}

#[cfg(test)]
mod tests {
    use super::{build_discovery_prompt, run_discovery, DimensionAssessment, DiscoveryAssessment};
    use crate::errors::GardenerError;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use crate::types::{AgentKind, RuntimeScope, WorkerState};
    use std::path::PathBuf;

    fn discovery_with_grade(grade: &str) -> DiscoveryAssessment {
        DiscoveryAssessment {
            agent_steering: DimensionAssessment {
                grade: grade.to_string(),
                summary: "agent steering".to_string(),
                issues: vec!["agent issue".to_string()],
                strengths: vec!["agent strength".to_string()],
            },
            knowledge_accessible: DimensionAssessment {
                grade: grade.to_string(),
                summary: "knowledge".to_string(),
                issues: Vec::new(),
                strengths: vec!["knowledge source".to_string()],
            },
            mechanical_guardrails: DimensionAssessment {
                grade: grade.to_string(),
                summary: "guardrails".to_string(),
                issues: vec!["guardrail issue".to_string()],
                strengths: Vec::new(),
            },
            local_feedback_loop: DimensionAssessment {
                grade: grade.to_string(),
                summary: "feedback".to_string(),
                issues: Vec::new(),
                strengths: vec!["feedback loop".to_string()],
            },
            coverage_signal: DimensionAssessment {
                grade: grade.to_string(),
                summary: "coverage".to_string(),
                issues: Vec::new(),
                strengths: Vec::new(),
            },
            overall_readiness_score: 76,
            overall_readiness_grade: grade.to_string(),
            primary_gap: "knowledge_accessible".to_string(),
            notable_findings: "stable".to_string(),
            scope_notes: "note".to_string(),
        }
    }

    fn discovery_envelope(assessment: &DiscoveryAssessment) -> String {
        let body = serde_json::json!({
            "schema_version": 1,
            "state": WorkerState::Seeding,
            "payload": {
                "gardener_output": assessment
            },
        });
        format!(
            "noise\n<<GARDENER_JSON_START>>{}<<GARDENER_JSON_END>>\n",
            serde_json::to_string(&body).expect("serializable discovery envelope for test")
        )
    }

    fn scope(workdir: &str, repo_root: Option<&str>) -> RuntimeScope {
        RuntimeScope {
            process_cwd: PathBuf::from(workdir),
            repo_root: repo_root.map(PathBuf::from),
            working_dir: PathBuf::from(workdir),
        }
    }

    #[test]
    fn build_discovery_prompt_includes_working_and_repo_root() {
        let prompt = build_discovery_prompt(&scope("/tmp/repo", Some("/tmp/repo")));
        assert!(prompt.contains("WORKING DIRECTORY: /tmp/repo"));
        assert!(prompt.contains("REPOSITORY ROOT: /tmp/repo"));
        assert!(!prompt.contains("scoped run; include root-level signals in scope_notes"));
    }

    #[test]
    fn build_discovery_prompt_includes_scoped_note_when_root_differs() {
        let prompt = build_discovery_prompt(&scope("/tmp/task", Some("/tmp/repo")));
        assert!(prompt.contains("WORKING DIRECTORY: /tmp/task"));
        assert!(prompt.contains("REPOSITORY ROOT: /tmp/repo"));
        assert!(prompt.contains("Note: scoped run; include root-level signals in scope_notes"));
    }

    #[test]
    fn run_discovery_success_for_codex() {
        let runner = FakeProcessRunner::default();
        let expected = discovery_with_grade("A");
        let expected_prompt = build_discovery_prompt(&scope("/tmp/repo", Some("/tmp/repo")));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: discovery_envelope(&expected),
            stderr: String::new(),
        }));

        let result = run_discovery(
            &runner,
            &scope("/tmp/repo", Some("/tmp/repo")),
            AgentKind::Codex,
            "gpt-4o",
            3,
        )
        .expect("discovery");

        let req = &runner.spawned()[0];
        assert_eq!(req.program, "codex");
        assert_eq!(
            req.args,
            vec![
                "exec".to_string(),
                "--json".to_string(),
                "--model".to_string(),
                "gpt-4o".to_string(),
                "--max-turns".to_string(),
                "3".to_string(),
                expected_prompt
            ]
        );
        assert_eq!(result.overall_readiness_grade, "A");
    }

    #[test]
    fn run_discovery_success_for_claude() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: discovery_envelope(&discovery_with_grade("B")),
            stderr: String::new(),
        }));

        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/tmp/repo/sub"),
            repo_root: Some(PathBuf::from("/tmp/repo")),
            working_dir: PathBuf::from("/tmp/repo/sub"),
        };
        let assessment =
            run_discovery(&runner, &scope, AgentKind::Claude, "sonnet", 2).expect("discovery");

        let req = &runner.spawned()[0];
        assert_eq!(req.program, "claude");
        assert_eq!(
            req.args,
            vec![
                "-p".to_string(),
                build_discovery_prompt(&scope),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--model".to_string(),
                "sonnet".to_string()
            ]
        );
        assert_eq!(assessment.overall_readiness_grade, "B");
    }

    #[test]
    fn run_discovery_returns_process_error_when_runner_fails() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "boom".to_string(),
        }));

        let error = run_discovery(
            &runner,
            &scope("/tmp/repo", Some("/tmp/repo")),
            AgentKind::Codex,
            "gpt-4o",
            1,
        )
        .expect_err("expected failure");
        assert!(matches!(error, GardenerError::Process(msg) if msg == "boom"));
    }

    #[test]
    fn run_discovery_returns_output_error_on_invalid_envelope() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "not an envelope".to_string(),
            stderr: String::new(),
        }));

        let error = run_discovery(
            &runner,
            &scope("/tmp/repo", Some("/tmp/repo")),
            AgentKind::Codex,
            "gpt-4o",
            1,
        )
        .expect_err("invalid envelope should fail");

        assert!(matches!(error, GardenerError::OutputEnvelope(_)));
    }
}
