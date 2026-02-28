ALTER TABLE backlog_tasks RENAME TO backlog_tasks_v2;

CREATE TABLE backlog_tasks (
    task_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    details TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    priority TEXT NOT NULL CHECK(priority IN ('P0', 'P1', 'P2')),
    status TEXT NOT NULL CHECK(status IN ('ready', 'leased', 'in_progress', 'complete', 'failed', 'unresolved')),
    last_updated INTEGER NOT NULL,
    lease_owner TEXT,
    lease_expires_at INTEGER,
    source TEXT NOT NULL,
    related_pr INTEGER,
    related_branch TEXT,
    rationale TEXT NOT NULL DEFAULT '',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

INSERT INTO backlog_tasks (
    task_id, kind, title, details, scope_key, priority, status, last_updated, lease_owner,
    lease_expires_at, source, related_pr, related_branch, rationale, attempt_count, created_at
)
SELECT
    task_id, kind, title, details, scope_key, priority, status, last_updated, lease_owner,
    lease_expires_at, source, related_pr, related_branch, rationale, attempt_count, created_at
FROM backlog_tasks_v2;

DROP TABLE backlog_tasks_v2;

CREATE INDEX IF NOT EXISTS idx_backlog_claim_order
    ON backlog_tasks(priority, status, last_updated, created_at);

CREATE INDEX IF NOT EXISTS idx_backlog_lease_expiry
    ON backlog_tasks(status, lease_expires_at);
