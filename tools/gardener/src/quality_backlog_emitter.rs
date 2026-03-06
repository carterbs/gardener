use crate::backlog_store::{BacklogStore, NewTask};
use crate::errors::GardenerError;
use crate::quality_assessment_types::StructuralDeficiency;
use crate::task_identity::TaskKind;

/// Convert structural deficiencies into backlog tasks via upsert.
///
/// Returns the list of task IDs that were upserted (one per deficiency).
pub fn emit_deficiency_tasks(
    store: &BacklogStore,
    deficiencies: &[StructuralDeficiency],
) -> Result<Vec<String>, GardenerError> {
    let mut task_ids = Vec::with_capacity(deficiencies.len());
    for d in deficiencies {
        let task = NewTask {
            kind: TaskKind::QualityGap,
            title: d.suggested_task_title.clone(),
            details: d.suggested_task_details.clone(),
            rationale: d.description.clone(),
            scope_key: format!(
                "quality:{}:{}",
                d.domain.as_deref().unwrap_or("repo"),
                d.category.as_str(),
            ),
            priority: d.severity,
            source: "quality-grading".to_string(),
            related_pr: None,
            related_branch: None,
        };
        let result = store.upsert_task(task)?;
        task_ids.push(result.task_id);
    }
    Ok(task_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::priority::Priority;
    use crate::quality_assessment_types::DeficiencyCategory;
    use tempfile::TempDir;

    fn temp_store() -> (BacklogStore, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join("backlog.sqlite");
        (BacklogStore::open(&db_path).expect("open temp store"), dir)
    }

    #[test]
    fn emit_creates_tasks_for_each_deficiency() {
        let (store, _dir) = temp_store();
        let deficiencies = vec![
            StructuralDeficiency {
                description: "No tests for auth".to_string(),
                domain: Some("auth".to_string()),
                category: DeficiencyCategory::CoverageGap,
                severity: Priority::P0,
                suggested_task_title: "Add auth tests".to_string(),
                suggested_task_details: "Write unit tests for auth module".to_string(),
            },
            StructuralDeficiency {
                description: "Missing linter config".to_string(),
                domain: None,
                category: DeficiencyCategory::MissingTooling,
                severity: Priority::P2,
                suggested_task_title: "Add linter".to_string(),
                suggested_task_details: "Configure clippy lints".to_string(),
            },
        ];

        let ids = emit_deficiency_tasks(&store, &deficiencies).expect("emit should succeed");
        assert_eq!(ids.len(), 2);

        // Verify tasks exist in the store
        let tasks = store.list_tasks().expect("list tasks");
        assert_eq!(tasks.len(), 2);

        // Verify scope keys are constructed correctly
        let auth_task = tasks
            .iter()
            .find(|t| t.title == "Add auth tests")
            .expect("auth task");
        assert_eq!(auth_task.scope_key, "quality:auth:coverage-gap");
        assert_eq!(auth_task.source, "quality-grading");

        let repo_task = tasks
            .iter()
            .find(|t| t.title == "Add linter")
            .expect("linter task");
        assert_eq!(repo_task.scope_key, "quality:repo:missing-tooling");
    }

    #[test]
    fn emit_empty_deficiencies_returns_empty_vec() {
        let (store, _dir) = temp_store();
        let ids = emit_deficiency_tasks(&store, &[]).expect("emit should succeed");
        assert!(ids.is_empty());
    }

    #[test]
    fn emit_upsert_is_idempotent() {
        let (store, _dir) = temp_store();
        let deficiencies = vec![StructuralDeficiency {
            description: "No tests".to_string(),
            domain: Some("core".to_string()),
            category: DeficiencyCategory::CoverageGap,
            severity: Priority::P1,
            suggested_task_title: "Add core tests".to_string(),
            suggested_task_details: "Write tests for core module".to_string(),
        }];

        let ids_first = emit_deficiency_tasks(&store, &deficiencies).expect("first emit");
        let ids_second = emit_deficiency_tasks(&store, &deficiencies).expect("second emit");

        // Same task IDs since scope_key + kind are the same
        assert_eq!(ids_first, ids_second);

        // Only one task in the store
        let tasks = store.list_tasks().expect("list tasks");
        assert_eq!(tasks.len(), 1);
    }
}
