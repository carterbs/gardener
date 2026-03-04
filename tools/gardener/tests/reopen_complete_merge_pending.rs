use gardener::backlog_store::{BacklogStore, NewTask, TaskStatus};
use gardener::priority::Priority;
use gardener::task_identity::TaskKind;
use tempfile::TempDir;

fn make_task(title: &str) -> NewTask {
    NewTask {
        kind: TaskKind::Feature,
        title: title.to_string(),
        details: "test details".to_string(),
        rationale: "test".to_string(),
        scope_key: "test".to_string(),
        priority: Priority::P1,
        source: "test".to_string(),
        related_pr: Some(42),
        related_branch: Some("gardener/test-branch".to_string()),
    }
}

#[test]
fn reopen_complete_task_with_open_pr() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("backlog.sqlite");
    let store = BacklogStore::open(&db_path).expect("open store");

    // Insert a task and move it through: ready → in_progress → complete
    let task = store.upsert_task(make_task("feat: test merge")).expect("upsert");
    let task_id = task.task_id.clone();

    let claimed = store
        .claim_next("worker-1", 900)
        .expect("claim")
        .expect("should claim task");
    assert_eq!(claimed.task_id, task_id);

    store.mark_complete(&task_id, "worker-1").expect("mark complete");

    // Verify task is complete
    let task = store.get_task(&task_id).expect("get").expect("exists");
    assert_eq!(task.status, TaskStatus::Complete);

    // Reopen it to merge_pending (simulating startup reconciliation
    // discovering an open PR for a completed task)
    let changed = store
        .reopen_complete_to_merge_pending(&task_id)
        .expect("reopen");
    assert!(changed, "task should have been re-opened");

    // Verify task is now merge_pending
    let task = store.get_task(&task_id).expect("get").expect("exists");
    assert_eq!(task.status, TaskStatus::MergePending);
    assert!(task.lease_owner.is_none(), "lease should be cleared");

    // Merge worker should be able to claim it
    let merge_claimed = store
        .claim_merge_pending("merge-worker")
        .expect("claim merge")
        .expect("should find task");
    assert_eq!(merge_claimed.task_id, task_id);
}

#[test]
fn reopen_noop_for_non_complete_task() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("backlog.sqlite");
    let store = BacklogStore::open(&db_path).expect("open store");

    let task = store.upsert_task(make_task("feat: ready task")).expect("upsert");
    let task_id = task.task_id.clone();

    // Task is ready, not complete — reopen should be a no-op
    let changed = store
        .reopen_complete_to_merge_pending(&task_id)
        .expect("reopen");
    assert!(!changed, "should not change a ready task");

    let task = store.get_task(&task_id).expect("get").expect("exists");
    assert_eq!(task.status, TaskStatus::Ready);
}
