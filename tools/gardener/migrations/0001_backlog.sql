CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS backlog_tasks (
    task_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    details TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    priority TEXT NOT NULL CHECK(priority IN ('P0', 'P1', 'P2')),
    status TEXT NOT NULL CHECK(status IN ('ready', 'leased', 'in_progress', 'complete', 'failed')),
    last_updated INTEGER NOT NULL,
    lease_owner TEXT,
    lease_expires_at INTEGER,
    source TEXT NOT NULL,
    related_pr INTEGER,
    related_branch TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_backlog_claim_order
    ON backlog_tasks(priority, status, last_updated, created_at);

CREATE INDEX IF NOT EXISTS idx_backlog_lease_expiry
    ON backlog_tasks(status, lease_expires_at);
