use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use tempfile::TempDir;

use crate::logging::append_run_log;
use super::logging::db_err;
use super::queries::{row_to_task, task_kind_from_db};
use super::{BacklogStore, NewTask, TaskStatus};
use crate::priority::Priority;
use crate::task_identity::{compute_task_id, TaskIdentity, TaskKind};

fn temp_store() -> (BacklogStore, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("backlog.sqlite");
    (BacklogStore::open(&db).expect("open store"), dir)
}

fn task(title: &str, priority: Priority) -> NewTask {
    NewTask {
        kind: TaskKind::Feature,
        title: title.to_string(),
        details: "details".to_string(),
        rationale: String::new(),
        scope_key: "domain:core".to_string(),
        priority,
        source: "test".to_string(),
        related_pr: None,
        related_branch: None,
    }
}

#[test]
fn upsert_dedupes_and_upgrades_priority() {
    let (store, _dir) = temp_store();

    let first = store
        .upsert_task(task("Normalize scheduler order", Priority::P2))
        .expect("insert");
    let second = store
        .upsert_task(task("  normalize   scheduler order  ", Priority::P0))
        .expect("upsert");

    assert_eq!(first.task_id, second.task_id);
    assert_eq!(second.priority, Priority::P0);

    let tasks = store.list_tasks().expect("list");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].priority, Priority::P0);
}

#[test]
fn lower_priority_reinsert_does_not_downgrade() {
    let (store, _dir) = temp_store();
    let _ = store
        .upsert_task(task("Fix lease collision", Priority::P0))
        .expect("insert");
    let row = store
        .upsert_task(task("fix lease collision", Priority::P2))
        .expect("upsert");
    assert_eq!(row.priority, Priority::P0);
}

#[test]
fn claim_is_priority_ordered_fifo_by_last_updated() {
    let (store, _dir) = temp_store();
    let _ = store
        .upsert_task(task("task-1", Priority::P1))
        .expect("insert 1");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let _ = store
        .upsert_task(task("task-2", Priority::P0))
        .expect("insert 2");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let _ = store
        .upsert_task(task("task-3", Priority::P0))
        .expect("insert 3");

    let first = store
        .claim_next("worker-a", 60)
        .expect("claim")
        .expect("task");
    let second = store
        .claim_next("worker-b", 60)
        .expect("claim")
        .expect("task");
    let third = store
        .claim_next("worker-c", 60)
        .expect("claim")
        .expect("task");

    assert_eq!(first.title, "task-2");
    assert_eq!(second.title, "task-3");
    assert_eq!(third.title, "task-1");
}

#[test]
fn claim_prioritizes_retries_within_same_priority() {
    let (store, _dir) = temp_store();
    let first = store
        .upsert_task(task("retry-me-first", Priority::P1))
        .expect("seed retry");

    let leased = store
        .claim_next("worker-a", 60)
        .expect("claim retry")
        .expect("retry row");
    assert_eq!(leased.task_id, first.task_id);
    let transitioned = store
        .mark_in_progress(&first.task_id, "worker-a")
        .expect("mark in progress");
    assert!(transitioned);
    let recovered = store.recover_stale_leases(i64::MAX).expect("recover stale");
    assert_eq!(recovered, 1);
    std::thread::sleep(std::time::Duration::from_millis(5));
    let _second = store
        .upsert_task(task("fresh-task", Priority::P1))
        .expect("seed fresh");

    let claimed = store
        .claim_next("worker-b", 60)
        .expect("claim after retry")
        .expect("task");
    assert_eq!(claimed.task_id, first.task_id);
    assert_eq!(claimed.attempt_count, 2);
}

#[test]
fn concurrent_claims_never_return_same_task() {
    let (store, _dir) = temp_store();
    let store = Arc::new(store);
    for idx in 0..25 {
        let _ = store
            .upsert_task(task(&format!("task-{idx}"), Priority::P1))
            .expect("seed task");
    }

    let mut joins = Vec::new();
    for worker in 0..25 {
        let store = Arc::clone(&store);
        joins.push(thread::spawn(move || {
            store
                .claim_next(&format!("worker-{worker}"), 60)
                .expect("claim")
                .map(|task| task.task_id)
        }));
    }

    let mut claimed = Vec::new();
    for join in joins {
        if let Some(task_id) = join.join().expect("join") {
            claimed.push(task_id);
        }
    }

    let unique = claimed.iter().cloned().collect::<HashSet<_>>();
    assert_eq!(claimed.len(), unique.len());
    assert_eq!(claimed.len(), 25);
}

#[test]
fn stale_recovery_requeues_in_progress_and_expired_leases() {
    let (store, _dir) = temp_store();
    let row = store
        .upsert_task(task("recover-me", Priority::P1))
        .expect("seed");

    let leased = store
        .claim_next("worker", 1)
        .expect("claim")
        .expect("leased row");
    assert_eq!(leased.status, TaskStatus::Leased);

    let transitioned = store
        .mark_in_progress(&row.task_id, "worker")
        .expect("in progress");
    assert!(transitioned);

    let recovered = store.recover_stale_leases(i64::MAX).expect("recover");
    assert_eq!(recovered, 1);

    let round_trip = store.get_task(&row.task_id).expect("fetch").expect("task");
    assert_eq!(round_trip.status, TaskStatus::Ready);
    assert_eq!(round_trip.lease_owner, None);
    assert_eq!(round_trip.lease_expires_at, None);
}

#[test]
fn stale_recovery_skips_in_progress_with_live_lease() {
    let (store, _dir) = temp_store();
    let row = store
        .upsert_task(task("active-work", Priority::P1))
        .expect("seed");
    let leased = store
        .claim_next("worker-a", 3600)
        .expect("claim")
        .expect("leased row");
    assert_eq!(leased.status, TaskStatus::Leased);
    let transitioned = store
        .mark_in_progress(&row.task_id, "worker-a")
        .expect("in progress");
    assert!(transitioned);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time is before UNIX epoch")
        .as_millis() as i64
        + 1000;
    let recovered = store.recover_stale_leases(now).expect("recover");
    assert_eq!(
        recovered, 0,
        "in_progress task with live lease must not be recovered"
    );

    let round_trip = store.get_task(&row.task_id).expect("fetch").expect("task");
    assert_eq!(round_trip.status, TaskStatus::InProgress);
    assert_eq!(round_trip.lease_owner.as_deref(), Some("worker-a"));
}

#[test]
fn mark_complete_requires_owner_match() {
    let (store, _dir) = temp_store();
    let row = store
        .upsert_task(task("complete-me", Priority::P1))
        .expect("seed");
    let _ = store.claim_next("worker-a", 60).expect("claim");

    let denied = store
        .mark_complete(&row.task_id, "worker-b")
        .expect("mismatch");
    assert!(!denied);

    let allowed = store
        .mark_complete(&row.task_id, "worker-a")
        .expect("owner match");
    assert!(allowed);

    let task = store.get_task(&row.task_id).expect("fetch").expect("row");
    assert_eq!(task.status, TaskStatus::Complete);
}

#[test]
fn reopen_complete_task_to_merge_pending() {
    let (store, _dir) = temp_store();
    let row = store
        .upsert_task(task("merge-reopen-me", Priority::P1))
        .expect("seed");

    let _ = store.claim_next("worker-a", 60).expect("claim");
    let in_progress = store
        .mark_in_progress(&row.task_id, "worker-a")
        .expect("in progress");
    assert!(in_progress);
    let complete = store
        .mark_complete(&row.task_id, "worker-a")
        .expect("complete");
    assert!(complete);

    let reopened = store
        .reopen_complete_to_merge_pending(&row.task_id)
        .expect("reopen");
    assert!(reopened);

    let task = store.get_task(&row.task_id).expect("fetch").expect("row");
    assert_eq!(task.status, TaskStatus::MergePending);
    assert_eq!(task.lease_owner, None);
    assert_eq!(task.lease_expires_at, None);
}

#[test]
fn set_merge_pending_to_ready_demotes_poisoned_merge_queue_rows() {
    let (store, _dir) = temp_store();
    let row = store
        .upsert_task(task("merge-poison-row", Priority::P1))
        .expect("seed");

    let _ = store.claim_next("worker-a", 60).expect("claim");
    let in_progress = store
        .mark_in_progress(&row.task_id, "worker-a")
        .expect("in progress");
    assert!(in_progress);
    let merge_pending = store
        .mark_merge_pending(&row.task_id, "worker-a")
        .expect("to merge pending");
    assert!(merge_pending);

    let demoted = store
        .set_merge_pending_to_ready(&row.task_id)
        .expect("demote merge pending to ready");
    assert!(demoted);

    let task = store.get_task(&row.task_id).expect("fetch").expect("row");
    assert_eq!(task.status, TaskStatus::Ready);
    assert_eq!(task.lease_owner, None);
    assert_eq!(task.lease_expires_at, None);
}

#[test]
fn mark_unresolved_requires_owner_match() {
    let (store, _dir) = temp_store();
    let row = store
        .upsert_task(task("unresolved-me", Priority::P1))
        .expect("seed");
    let _ = store.claim_next("worker-a", 60).expect("claim");

    let denied = store
        .mark_unresolved(&row.task_id, "worker-b")
        .expect("mismatch");
    assert!(!denied);

    let allowed = store
        .mark_unresolved(&row.task_id, "worker-a")
        .expect("owner match");
    assert!(allowed);

    let task = store.get_task(&row.task_id).expect("fetch").expect("row");
    assert_eq!(task.status, TaskStatus::Unresolved);
    assert_eq!(task.lease_owner, None);
    assert_eq!(task.lease_expires_at, None);
}

#[test]
fn task_identity_contract_matches_store_ids() {
    let input = task("Identity Task", Priority::P1);
    let expected = compute_task_id(TaskIdentity {
        kind: TaskKind::Feature,
        title: "identity task".to_string(),
        scope_key: "domain:core".to_string(),
        related_pr: None,
        related_branch: None,
    });

    let (store, _dir) = temp_store();
    let row = store.upsert_task(input).expect("insert");
    assert_eq!(row.task_id, expected);
}

#[test]
fn covers_conversion_and_error_paths() {
    append_run_log("debug", "backlog_store.tests.covers_conversion", serde_json::json!({}));
    assert_eq!(TaskStatus::Ready.as_str(), "ready");
    assert_eq!(TaskStatus::Leased.as_str(), "leased");
    assert_eq!(TaskStatus::InProgress.as_str(), "in_progress");
    assert_eq!(TaskStatus::Complete.as_str(), "complete");
    assert_eq!(TaskStatus::Failed.as_str(), "failed");
    assert_eq!(TaskStatus::Unresolved.as_str(), "unresolved");
    assert_eq!(TaskStatus::from_db("failed"), Some(TaskStatus::Failed));
    assert_eq!(TaskStatus::from_db("unresolved"), Some(TaskStatus::Unresolved));
    assert_eq!(TaskStatus::from_db("unknown"), None);

    let (store, _dir) = temp_store();
    assert!(store.db_path().ends_with("backlog.sqlite"));
    assert_eq!(task_kind_from_db("bugfix"), Some(TaskKind::Bugfix));
    assert_eq!(task_kind_from_db("maintenance"), Some(TaskKind::Maintenance));
    assert_eq!(task_kind_from_db("infra"), Some(TaskKind::Infra));
    assert_eq!(task_kind_from_db("nope"), None);

    let _ = store
        .upsert_task(NewTask {
            kind: TaskKind::Bugfix,
            title: "b".to_string(),
            details: String::new(),
            rationale: String::new(),
            scope_key: "global".to_string(),
            priority: Priority::P1,
            source: "t".to_string(),
            related_pr: None,
            related_branch: None,
        })
        .expect("bugfix insert");
    let _ = store
        .upsert_task(NewTask {
            kind: TaskKind::Maintenance,
            title: "m".to_string(),
            details: String::new(),
            rationale: String::new(),
            scope_key: "global".to_string(),
            priority: Priority::P2,
            source: "t".to_string(),
            related_pr: None,
            related_branch: None,
        })
        .expect("maintenance insert");
    let _ = store
        .upsert_task(NewTask {
            kind: TaskKind::Infra,
            title: "i".to_string(),
            details: String::new(),
            rationale: String::new(),
            scope_key: "global".to_string(),
            priority: Priority::P0,
            source: "t".to_string(),
            related_pr: None,
            related_branch: None,
        })
        .expect("infra insert");

    let reopened = BacklogStore::open(store.db_path()).expect("reopen");
    assert!(reopened.list_tasks().expect("reopened list").len() >= 3);

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let base = std::env::temp_dir().join(format!("gardener-backlog-file-parent-{nonce}"));
    std::fs::create_dir_all(&base).expect("mkdir");
    let parent_file = base.join("not-a-dir");
    std::fs::write(&parent_file, "x").expect("file");
    let invalid_db = parent_file.join("db.sqlite");
    let err = match BacklogStore::open(&invalid_db) {
        Ok(_) => panic!("parent should fail"),
        Err(err) => err,
    };
    assert!(matches!(err, crate::errors::GardenerError::Database(_)));

    let conversion_conn = Connection::open_in_memory().expect("open memory");
    let bad_kind = conversion_conn.query_row(
        "SELECT 'id', 'invalid', 'title', '', 'global', 'P1', 'ready', 1, NULL, NULL, 'src', NULL, NULL, '', 0, 1",
        [],
        row_to_task,
    );
    assert!(bad_kind.is_err());

    let bad_priority = conversion_conn.query_row(
        "SELECT 'id', 'feature', 'title', '', 'global', 'PX', 'ready', 1, NULL, NULL, 'src', NULL, NULL, '', 0, 1",
        [],
        row_to_task,
    );
    assert!(bad_priority.is_err());

    let bad_status = conversion_conn.query_row(
        "SELECT 'id', 'feature', 'title', '', 'global', 'P1', 'unknown', 1, NULL, NULL, 'src', NULL, NULL, '', 0, 1",
        [],
        row_to_task,
    );
    assert!(bad_status.is_err());

    let converted = db_err(rusqlite::Error::InvalidQuery);
    assert!(matches!(
        converted,
        crate::errors::GardenerError::Database(_)
    ));
}

#[test]
fn count_tasks_by_priority_excludes_complete() {
    let (store, _dir) = temp_store();
    let ready_p1 = store
        .upsert_task(task("ready p1", Priority::P1))
        .expect("insert ready p1");
    let _ = store
        .upsert_task(task("ready p2", Priority::P2))
        .expect("insert ready p2");
    let complete = store
        .upsert_task(task("complete p0", Priority::P0))
        .expect("insert complete candidate");
    let claimed = store
        .claim_next("worker-1", 60)
        .expect("claim complete candidate")
        .expect("claimed task");
    assert_eq!(claimed.task_id, complete.task_id);
    let moved = store
        .mark_in_progress(&complete.task_id, "worker-1")
        .expect("mark in progress");
    assert!(moved);
    let completed = store
        .mark_complete(&complete.task_id, "worker-1")
        .expect("mark complete");
    assert!(completed);

    let _ = ready_p1;
    let (p0, p1, p2) = store.count_tasks_by_priority().expect("count");
    assert_eq!(p0, 0);
    assert_eq!(p1, 1);
    assert_eq!(p2, 1);
}

#[test]
fn drop_flushes_pending_writes() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("backlog.sqlite");
    {
        let store = BacklogStore::open(&db).expect("open store");
        store
            .upsert_task(task("survive-drop", Priority::P1))
            .expect("upsert");
    }
    let reopened = BacklogStore::open(&db).expect("reopen");
    let tasks = reopened.list_tasks().expect("list");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "survive-drop");
}

#[test]
fn open_rejects_zero_byte_file() {
    append_run_log(
        "debug",
        "backlog_store.tests.open_rejects_zero_byte_file",
        serde_json::json!({}),
    );
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("backlog.sqlite");
    std::fs::write(&db, b"").expect("create zero-byte file");
    match BacklogStore::open(&db) {
        Err(crate::errors::GardenerError::Database(msg)) => {
            assert!(msg.contains("0 bytes"), "unexpected message: {msg}");
        }
        Err(e) => panic!("expected Database error, got: {e}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[test]
fn open_rejects_corrupt_file() {
    append_run_log(
        "debug",
        "backlog_store.tests.open_rejects_corrupt_file",
        serde_json::json!({}),
    );
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("backlog.sqlite");
    std::fs::write(&db, b"this is not a sqlite database at all").expect("write garbage");
    match BacklogStore::open(&db) {
        Err(crate::errors::GardenerError::Database(_)) => {}
        Err(e) => panic!("expected Database error, got: {e}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[test]
fn list_backlog_tasks_hides_merge_pending() {
    let (store, _dir) = temp_store();

    let merge_task = store
        .upsert_task(task("merge queue task", Priority::P0))
        .expect("insert merge queue task");
    let _ = store
        .upsert_task(task("backlog task", Priority::P1))
        .expect("insert backlog task");

    let claimed = store
        .claim_next("worker", 60)
        .expect("claim")
        .expect("claimed task");
    assert_eq!(claimed.task_id, merge_task.task_id);

    let in_progress = store
        .mark_in_progress(&merge_task.task_id, "worker")
        .expect("mark in progress");
    assert!(in_progress);
    let moved_to_queue = store
        .mark_merge_pending(&merge_task.task_id, "worker")
        .expect("mark merge pending");
    assert!(moved_to_queue);

    let tasks = store.list_backlog_tasks().expect("list backlog tasks");
    assert!(tasks
        .iter()
        .all(|task| task.status != TaskStatus::MergePending));
    assert!(tasks.iter().any(|task| task.title == "backlog task"));
    assert!(!tasks.iter().any(|task| task.title == "merge queue task"));
}

#[test]
fn insert_and_list_rejected_seeds_round_trip() {
    use crate::seed_runner::SeedTask;

    let (store, _dir) = temp_store();
    let task = SeedTask {
        title: "Fix flaky tests".to_string(),
        details: "Stabilize intermittent CI failures".to_string(),
        rationale: "Reduces agent confusion from false negatives".to_string(),
        domain: "testing".to_string(),
        priority: "P1".to_string(),
    };

    store
        .insert_rejected_seed(&task, Some("too vague"))
        .expect("insert with reason");

    let seeds = store.list_rejected_seeds().expect("list");
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].title, "Fix flaky tests");
    assert_eq!(seeds[0].details, "Stabilize intermittent CI failures");
    assert_eq!(seeds[0].rejection_reason, "too vague");
    assert_eq!(seeds[0].domain, "testing");
}

#[test]
fn insert_rejected_seed_without_reason() {
    use crate::seed_runner::SeedTask;

    let (store, _dir) = temp_store();
    let task = SeedTask {
        title: "Add logging".to_string(),
        details: "Improve observability".to_string(),
        rationale: "Helps debug agent runs".to_string(),
        domain: "infra".to_string(),
        priority: "P2".to_string(),
    };

    store
        .insert_rejected_seed(&task, None)
        .expect("insert without reason");

    let seeds = store.list_rejected_seeds().expect("list");
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].title, "Add logging");
    assert_eq!(seeds[0].rejection_reason, "");
}

#[test]
fn insert_rejected_seed_deduplicates_by_title_and_domain() {
    use crate::seed_runner::SeedTask;

    let (store, _dir) = temp_store();
    let task = SeedTask {
        title: "Normalize paths".to_string(),
        details: "First version".to_string(),
        rationale: "r1".to_string(),
        domain: "core".to_string(),
        priority: "P1".to_string(),
    };

    store
        .insert_rejected_seed(&task, Some("first reason"))
        .expect("first insert");

    let updated = SeedTask {
        title: "Normalize paths".to_string(),
        details: "Updated version".to_string(),
        rationale: "r2".to_string(),
        domain: "core".to_string(),
        priority: "P0".to_string(),
    };

    store
        .insert_rejected_seed(&updated, Some("updated reason"))
        .expect("second insert");

    let seeds = store.list_rejected_seeds().expect("list");
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].details, "Updated version");
    assert_eq!(seeds[0].rejection_reason, "updated reason");
}

#[test]
fn list_rejected_seeds_caps_at_twenty() {
    use crate::seed_runner::SeedTask;

    let (store, _dir) = temp_store();

    for i in 0..25 {
        let task = SeedTask {
            title: format!("Rejected task {i}"),
            details: format!("Details for task {i}"),
            rationale: "r".to_string(),
            domain: format!("domain-{i}"),
            priority: "P1".to_string(),
        };
        store
            .insert_rejected_seed(&task, Some(&format!("reason {i}")))
            .expect("insert");
    }

    let seeds = store.list_rejected_seeds().expect("list");
    assert_eq!(seeds.len(), 20);
}
