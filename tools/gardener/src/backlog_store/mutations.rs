use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;

use crate::errors::GardenerError;
use crate::logging::append_run_log;

use super::logging::{compute_task_id_from_new_task, db_err};
use super::queries::{fetch_task, row_to_task};
use super::types::{
    BacklogTask, ManualTaskInput, NewTask, TaskMutation, TaskStatus, TaskUpdatePatch,
};
use super::types::StoreResult;

pub(super) fn upsert_task(conn: &Connection, task: &NewTask, now: i64) -> StoreResult<()> {
    append_run_log(
        "debug",
        "backlog_store.upsert_task.started",
        json!({
            "task_id": compute_task_id_from_new_task(task),
            "scope_key": task.scope_key,
        }),
    );
    let task_id = compute_task_id_from_new_task(task);
    conn.execute(
        "INSERT INTO backlog_tasks (
            task_id, kind, title, details, scope_key, priority, status, last_updated, lease_owner,
            lease_expires_at, source, related_pr, related_branch, rationale, attempt_count, created_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, 'ready', ?7, NULL, NULL, ?8, ?9, ?10, ?11, 0, ?12
        )
        ON CONFLICT(task_id) DO UPDATE SET
            title = excluded.title,
            details = excluded.details,
            scope_key = excluded.scope_key,
            priority = CASE
                WHEN CASE excluded.priority
                    WHEN 'P0' THEN 0
                    WHEN 'P1' THEN 1
                    ELSE 2
                END < CASE backlog_tasks.priority
                    WHEN 'P0' THEN 0
                    WHEN 'P1' THEN 1
                    ELSE 2
                END THEN excluded.priority
                ELSE backlog_tasks.priority
            END,
            status = CASE
                WHEN backlog_tasks.status IN ('leased', 'in_progress') THEN backlog_tasks.status
                ELSE 'ready'
            END,
            last_updated = excluded.last_updated,
            lease_owner = CASE
                WHEN backlog_tasks.status IN ('leased', 'in_progress') THEN backlog_tasks.lease_owner
                ELSE NULL
            END,
            lease_expires_at = CASE
                WHEN backlog_tasks.status IN ('leased', 'in_progress') THEN backlog_tasks.lease_expires_at
                ELSE NULL
            END,
            source = excluded.source,
            related_pr = excluded.related_pr,
            related_branch = excluded.related_branch,
            rationale = excluded.rationale",
        params![
            task_id,
            task.kind.as_str(),
            task.title,
            task.details,
            task.scope_key,
            task.priority.as_str(),
            now,
            task.source,
            task.related_pr,
            task.related_branch,
            task.rationale,
            now,
        ],
    )
    .map_err(db_err)?;
    Ok(())
}

pub(super) fn insert_manual_task(
    conn: &Connection,
    task: &ManualTaskInput,
    now: i64,
) -> StoreResult<()> {
    append_run_log(
        "debug",
        "backlog_store.insert_manual_task.started",
        json!({
            "task_id": task.task_id,
            "scope_key": task.scope_key,
            "status": task.status.as_str(),
        }),
    );
    conn.execute(
        "INSERT INTO backlog_tasks (
            task_id, kind, title, details, scope_key, priority, status, last_updated, lease_owner,
            lease_expires_at, source, related_pr, related_branch, rationale, attempt_count, created_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, ?9, ?10, ?11, ?12, 0, ?13
        )",
        params![
            task.task_id,
            task.kind.as_str(),
            task.title,
            task.details,
            task.scope_key,
            task.priority.as_str(),
            task.status.as_str(),
            now,
            task.source,
            task.related_pr,
            task.related_branch,
            task.rationale,
            now,
        ],
    )
    .map_err(db_err)?;
    Ok(())
}

pub(super) fn claim_next(
    conn: &mut Connection,
    lease_owner: &str,
    lease_expires_at: i64,
    now: i64,
) -> StoreResult<Option<BacklogTask>> {
    let tx = conn.transaction().map_err(db_err)?;
    let maybe = claim_next_in_tx(&tx, lease_owner, lease_expires_at, now)?;
    tx.commit().map_err(db_err)?;
    Ok(maybe)
}

fn claim_next_in_tx(
    tx: &Transaction<'_>,
    lease_owner: &str,
    lease_expires_at: i64,
    now: i64,
) -> StoreResult<Option<BacklogTask>> {
    append_run_log(
        "debug",
        "backlog_store.claim_next_in_tx.started",
        json!({
            "lease_owner": lease_owner,
            "lease_expires_at": lease_expires_at
        }),
    );
    let mut candidate = tx
        .prepare(
            "SELECT task_id
             FROM backlog_tasks
             WHERE status = 'ready'
             ORDER BY
                CASE priority WHEN 'P0' THEN 0 WHEN 'P1' THEN 1 ELSE 2 END,
                CASE WHEN attempt_count > 0 THEN 0 ELSE 1 END,
                attempt_count DESC,
                last_updated ASC,
                created_at ASC
             LIMIT 1",
        )
        .map_err(db_err)?;
    let Some(task_id) = candidate
        .query_row([], |row| row.get::<_, String>(0))
        .optional()
        .map_err(db_err)?
    else {
        return Ok(None);
    };

    let mut stmt = tx
        .prepare(
            "UPDATE backlog_tasks
             SET status = 'leased',
                 lease_owner = ?2,
                 lease_expires_at = ?3,
                 last_updated = ?4,
                 attempt_count = attempt_count + 1
             WHERE task_id = ?1 AND status = 'ready'
             RETURNING task_id, kind, title, details, scope_key, priority, status, last_updated,
                       lease_owner, lease_expires_at, source, related_pr, related_branch, rationale,
                       attempt_count, created_at",
        )
        .map_err(db_err)?;

    stmt.query_row(
        params![task_id, lease_owner, lease_expires_at, now],
        row_to_task,
    )
    .optional()
    .map_err(db_err)
}

pub(super) fn mark_in_progress(
    conn: &Connection,
    task_id: &str,
    lease_owner: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.mark_in_progress.started",
        json!({
            "task_id": task_id,
            "lease_owner": lease_owner,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'in_progress', last_updated = ?1
             WHERE task_id = ?2 AND status = 'leased' AND lease_owner = ?3",
            params![now, task_id, lease_owner],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn mark_complete(
    conn: &Connection,
    task_id: &str,
    lease_owner: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.mark_complete.started",
        json!({
            "task_id": task_id,
            "lease_owner": lease_owner,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'complete', lease_owner = NULL, lease_expires_at = NULL, last_updated = ?1
             WHERE task_id = ?2 AND lease_owner = ?3 AND status IN ('leased', 'in_progress', 'merge_pending')",
            params![now, task_id, lease_owner],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn release_lease(
    conn: &Connection,
    task_id: &str,
    lease_owner: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.release_lease.started",
        json!({
            "task_id": task_id,
            "lease_owner": lease_owner,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'ready', lease_owner = NULL, lease_expires_at = NULL, last_updated = ?1
             WHERE task_id = ?2 AND lease_owner = ?3 AND status IN ('leased', 'in_progress')",
            params![now, task_id, lease_owner],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn mark_unresolved(
    conn: &Connection,
    task_id: &str,
    lease_owner: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.mark_unresolved.started",
        json!({
            "task_id": task_id,
            "lease_owner": lease_owner,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'unresolved', lease_owner = NULL, lease_expires_at = NULL, last_updated = ?1
             WHERE task_id = ?2 AND lease_owner = ?3 AND status IN ('leased', 'in_progress')",
            params![now, task_id, lease_owner],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn set_unresolved_status(
    conn: &Connection,
    task_id: &str,
    status: TaskStatus,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.set_unresolved_status.started",
        json!({
            "task_id": task_id,
            "status": status.as_str(),
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = ?1, lease_owner = NULL, lease_expires_at = NULL, last_updated = ?2
             WHERE task_id = ?3 AND status = 'unresolved'",
            params![status.as_str(), now, task_id],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn set_merge_pending_to_ready(
    conn: &Connection,
    task_id: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.set_merge_pending_to_ready.started",
        json!({
            "task_id": task_id,
            "status": TaskStatus::Ready.as_str(),
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'ready', lease_owner = NULL, lease_expires_at = NULL, last_updated = ?1
             WHERE task_id = ?2 AND status = 'merge_pending'",
            params![now, task_id],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn clear_related_pr(conn: &Connection, task_id: &str) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.clear_related_pr.started",
        json!({
            "task_id": task_id,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET related_pr = NULL,
                 related_branch = NULL
             WHERE task_id = ?1",
            params![task_id],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn mark_merge_pending(
    conn: &Connection,
    task_id: &str,
    lease_owner: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.mark_merge_pending.started",
        json!({
            "task_id": task_id,
            "lease_owner": lease_owner,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'merge_pending', lease_owner = NULL, lease_expires_at = NULL, last_updated = ?1
             WHERE task_id = ?2 AND lease_owner = ?3 AND status = 'in_progress'",
            params![now, task_id, lease_owner],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn claim_merge_pending(
    conn: &Connection,
    merge_worker_id: &str,
    now: i64,
) -> StoreResult<Option<BacklogTask>> {
    append_run_log(
        "debug",
        "backlog_store.claim_merge_pending.started",
        json!({ "merge_worker_id": merge_worker_id }),
    );
    let task = conn
        .query_row(
            "UPDATE backlog_tasks
             SET status = 'in_progress', lease_owner = ?1, last_updated = ?2
             WHERE task_id = (
                 SELECT task_id FROM backlog_tasks
                 WHERE status = 'merge_pending'
                 ORDER BY last_updated ASC
                 LIMIT 1
             )
             RETURNING task_id, kind, title, details, scope_key, priority, status,
                       last_updated, lease_owner, lease_expires_at, source,
                       related_pr, related_branch, rationale, attempt_count, created_at",
            params![merge_worker_id, now],
            row_to_task,
        )
        .optional()
        .map_err(db_err)?;
    Ok(task)
}

pub(super) fn set_related_pr(
    conn: &Connection,
    task_id: &str,
    pr_number: i64,
    branch: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.set_related_pr.started",
        json!({
            "task_id": task_id,
            "pr_number": pr_number,
            "branch": branch,
        }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET related_pr = COALESCE(related_pr, ?2),
                 related_branch = COALESCE(related_branch, ?3),
                 last_updated = ?1
             WHERE task_id = ?4",
            params![now, pr_number, branch, task_id],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn promote_ready_with_pr(conn: &Connection, now: i64) -> StoreResult<usize> {
    append_run_log(
        "debug",
        "backlog_store.promote_ready_with_pr.started",
        json!({}),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'merge_pending', lease_owner = NULL, lease_expires_at = NULL, last_updated = ?1
             WHERE status = 'ready' AND related_pr IS NOT NULL",
            params![now],
        )
        .map_err(db_err)?;
    Ok(changed)
}

pub(super) fn reopen_complete_to_merge_pending(
    conn: &Connection,
    task_id: &str,
    now: i64,
) -> StoreResult<bool> {
    append_run_log(
        "debug",
        "backlog_store.reopen_complete_to_merge_pending.started",
        json!({ "task_id": task_id }),
    );
    let changed = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'merge_pending', lease_owner = NULL, lease_expires_at = NULL, last_updated = ?1
             WHERE task_id = ?2 AND status = 'complete'",
            params![now, task_id],
        )
        .map_err(db_err)?;
    Ok(changed > 0)
}

pub(super) fn recover_stale(conn: &Connection, now: i64) -> StoreResult<usize> {
    append_run_log(
        "debug",
        "backlog_store.recover_stale.started",
        json!({ "now": now }),
    );
    let non_pr_recovered = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'ready',
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 last_updated = ?1
             WHERE (status = 'merge_pending'
                OR ((status = 'in_progress' OR status = 'leased')
                    AND (lease_expires_at IS NULL OR lease_expires_at < ?1)))
                AND related_pr IS NULL",
            [now],
        )
        .map_err(db_err)?;
    let pr_recovered = conn
        .execute(
            "UPDATE backlog_tasks
             SET status = 'merge_pending',
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 last_updated = ?1
             WHERE (status = 'merge_pending'
                OR ((status = 'in_progress' OR status = 'leased')
                    AND (lease_expires_at IS NULL OR lease_expires_at < ?1)))
                AND related_pr IS NOT NULL",
            [now],
        )
        .map_err(db_err)?;
    Ok(non_pr_recovered + pr_recovered)
}

pub(super) fn update_task_metadata(
    conn: &Connection,
    task_id: &str,
    patch: &TaskUpdatePatch,
    now: i64,
) -> StoreResult<TaskMutation> {
    let before = fetch_task(conn, task_id)?
        .ok_or_else(|| GardenerError::Database(format!("backlog task not found: {task_id}")))?;

    if patch.status.is_none()
        && patch.rationale.is_none()
        && patch.related_pr.is_none()
        && patch.related_branch.is_none()
        && !patch.clear_lease
    {
        return Err(GardenerError::Cli(
            "update requires at least one change".to_string(),
        ));
    }

    let clear_lease = patch.clear_lease
        || matches!(
            patch.status,
            Some(status) if !matches!(status, TaskStatus::Leased | TaskStatus::InProgress)
        );

    let changed = patch
        .status
        .map(|status| status != before.status)
        .unwrap_or(false)
        || patch
            .rationale
            .as_ref()
            .map(|rationale| rationale != &before.rationale)
            .unwrap_or(false)
        || patch
            .related_pr
            .map(|related_pr| Some(related_pr) != before.related_pr)
            .unwrap_or(false)
        || patch
            .related_branch
            .as_ref()
            .map(|branch| Some(branch.as_str()) != before.related_branch.as_deref())
            .unwrap_or(false)
        || (clear_lease && (before.lease_owner.is_some() || before.lease_expires_at.is_some()));

    if changed {
        conn.execute(
            "UPDATE backlog_tasks
             SET status = COALESCE(?1, status),
                 rationale = COALESCE(?2, rationale),
                related_pr = CASE WHEN ?3 THEN ?4 ELSE related_pr END,
                related_branch = CASE WHEN ?5 THEN ?6 ELSE related_branch END,
                lease_owner = CASE WHEN ?7 THEN NULL ELSE lease_owner END,
                lease_expires_at = CASE WHEN ?7 THEN NULL ELSE lease_expires_at END,
                last_updated = ?8
             WHERE task_id = ?9",
            params![
                patch.status.map(TaskStatus::as_str),
                patch.rationale.as_deref(),
                patch.related_pr.is_some(),
                patch.related_pr,
                patch.related_branch.is_some(),
                patch.related_branch.as_deref(),
                clear_lease,
                now,
                task_id,
            ],
        )
        .map_err(db_err)?;
    }

    let after = fetch_task(conn, task_id)?.ok_or_else(|| {
        GardenerError::Database(format!("backlog task not found after update: {task_id}"))
    })?;

    Ok(TaskMutation {
        before,
        after,
        changed,
    })
}
