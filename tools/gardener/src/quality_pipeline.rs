use std::path::Path;

use crate::agent::factory::AdapterFactory;
use crate::backlog_store::BacklogStore;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::quality_assessment_runner::{
    run_assessment, QualityAssessmentConfig, QualityProgressEvent,
};
use crate::quality_backlog_emitter::emit_deficiency_tasks;
use crate::quality_grade_compute::{compute_grade_report, GradeReport};
use crate::quality_grade_renderer::render_grade_document_with_repo_wide;
use crate::runtime::ProcessRunner;
use serde_json::json;

/// Run the full quality grading pipeline: assess, grade, render, and emit backlog tasks.
///
/// Returns the rendered Markdown document and the computed grade report.
pub fn run_quality_pipeline(
    repo_path: &Path,
    factory: Option<&AdapterFactory>,
    process_runner: &dyn ProcessRunner,
    store: Option<&BacklogStore>,
    config: &QualityAssessmentConfig,
    on_progress: Option<&(dyn Fn(QualityProgressEvent) + Send + Sync)>,
) -> Result<(String, GradeReport), GardenerError> {
    append_run_log(
        "info",
        "quality_pipeline.started",
        json!({
            "repo_path": repo_path.display().to_string(),
            "has_factory": factory.is_some(),
            "has_store": store.is_some(),
        }),
    );

    // 1. Run assessment (handles evidence collection, agent/fallback internally)
    let (payload, _bundle, agent_used) =
        run_assessment(repo_path, factory, process_runner, config, on_progress)?;

    // Keep repo_wide for the renderer (before payload is consumed by compute_grade_report)
    let repo_wide = payload.repo_wide.clone();

    // 2. Determine assessed_by from actual outcome, not just factory presence
    let assessed_by = if agent_used {
        "agent"
    } else {
        "deterministic-fallback"
    };

    // 3. Compute grades
    let report = compute_grade_report(payload);

    // 4. Render document
    let document = render_grade_document_with_repo_wide(&report, &repo_wide, assessed_by);

    // 5. Emit backlog tasks
    if let Some(store) = store {
        match emit_deficiency_tasks(store, &report.deficiencies) {
            Ok(task_ids) => {
                append_run_log(
                    "info",
                    "quality_pipeline.backlog_emitted",
                    json!({
                        "task_count": task_ids.len(),
                    }),
                );
            }
            Err(e) => {
                append_run_log(
                    "warn",
                    "quality_pipeline.backlog_emit_failed",
                    json!({
                        "error": e.to_string(),
                    }),
                );
            }
        }
    }

    append_run_log(
        "info",
        "quality_pipeline.completed",
        json!({
            "assessed_by": assessed_by,
            "domain_count": report.domain_grades.len(),
            "deficiency_count": report.deficiencies.len(),
            "repo_grade": report.repo_grade.1.as_str(),
        }),
    );

    Ok((document, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::FakeProcessRunner;
    use tempfile::tempdir;

    #[test]
    fn pipeline_runs_without_factory_or_store() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create dir");
        std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write");

        let config = QualityAssessmentConfig::default();
        let runner = FakeProcessRunner::default();

        let (doc, report) = run_quality_pipeline(dir.path(), None, &runner, None, &config, None)
            .expect("should succeed");

        assert!(doc.contains("# Quality Grade Report"));
        assert!(!report.domain_grades.is_empty() || report.deficiencies.is_empty());
    }

    #[test]
    fn pipeline_emits_backlog_tasks_when_store_provided() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create dir");
        std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write");

        let db_path = dir.path().join("backlog.sqlite");
        let store = BacklogStore::open(&db_path).expect("open store");

        let config = QualityAssessmentConfig::default();
        let runner = FakeProcessRunner::default();

        let (_doc, report) =
            run_quality_pipeline(dir.path(), None, &runner, Some(&store), &config, None)
                .expect("should succeed");

        // If there are deficiencies, tasks should have been emitted
        if !report.deficiencies.is_empty() {
            let tasks = store.list_tasks().expect("list tasks");
            assert!(!tasks.is_empty());
        }
    }

    #[test]
    fn pipeline_document_contains_assessed_by() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("create dir");
        std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write");

        let config = QualityAssessmentConfig::default();
        let runner = FakeProcessRunner::default();

        let (doc, _report) = run_quality_pipeline(dir.path(), None, &runner, None, &config, None)
            .expect("should succeed");

        assert!(doc.contains("Assessed by: deterministic-fallback"));
    }
}
