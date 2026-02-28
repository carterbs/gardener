use crate::logging::append_run_log;
use std::path::Path;
use serde_json::json;

use crate::backlog_store::{BacklogStore, BacklogTask};
use crate::errors::GardenerError;

pub fn export_markdown_snapshot(
    store: &BacklogStore,
    output: impl AsRef<Path>,
) -> Result<String, GardenerError> {
    let tasks = store.list_tasks()?;
    append_run_log(
        "debug",
        "backlog_snapshot.export.started",
        json!({
            "path": output.as_ref().display().to_string(),
            "task_count": tasks.len(),
        }),
    );
    let rendered = render_markdown(&tasks);
    std::fs::write(output, rendered.as_bytes()).map_err(|e| GardenerError::Io(e.to_string()))?;
    Ok(rendered)
}

pub fn render_markdown(tasks: &[BacklogTask]) -> String {
    let mut out = String::new();
    out.push_str("# Gardener Backlog Snapshot\n\n");
    out.push_str("| Priority | Status | Title | Task ID | Updated |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");

    for task in tasks {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            task.priority.as_str(),
            task.status.as_str(),
            sanitize_cell(&task.title),
            task.task_id,
            task.last_updated
        ));
    }

    out
}

fn sanitize_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::backlog_store::{BacklogStore, NewTask};
    use crate::priority::Priority;
    use crate::task_identity::TaskKind;

    use super::{export_markdown_snapshot, render_markdown};

    #[test]
    fn snapshot_renders_expected_table() {
        let store_dir = tempdir().expect("dir");
        let db = store_dir.path().join("snapshot.sqlite");
        let store = BacklogStore::open(&db).expect("store");
        store
            .upsert_task(NewTask {
                kind: TaskKind::Feature,
                title: "First task".to_string(),
                details: String::new(),
                scope_key: "global".to_string(),
                priority: Priority::P1,
                source: "test".to_string(),
                related_pr: None,
                related_branch: None,
            })
            .expect("insert");

        let rendered = render_markdown(&store.list_tasks().expect("tasks"));
        assert!(rendered.contains("# Gardener Backlog Snapshot"));
        assert!(rendered.contains("| P1 | ready | First task |"));
    }

    #[test]
    fn exporter_writes_file() {
        let dir = tempdir().expect("dir");
        let db = dir.path().join("export.sqlite");
        let out = dir.path().join("backlog.md");
        let store = BacklogStore::open(&db).expect("store");

        let rendered = export_markdown_snapshot(&store, &out).expect("export");
        let disk = std::fs::read_to_string(&out).expect("read");

        assert_eq!(rendered, disk);
    }

    #[test]
    fn sanitizes_markdown_cells() {
        assert_eq!(super::sanitize_cell("a|b\nc"), "a\\|b c");
    }
}
