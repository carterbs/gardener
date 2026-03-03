CREATE TABLE IF NOT EXISTS rejected_seed_tasks (
    id              TEXT PRIMARY KEY,
    title           TEXT NOT NULL,
    details         TEXT NOT NULL,
    rationale       TEXT NOT NULL DEFAULT '',
    domain          TEXT NOT NULL DEFAULT 'infrastructure',
    priority        TEXT NOT NULL DEFAULT 'P1',
    rejection_reason TEXT NOT NULL DEFAULT '',
    rejected_at     INTEGER NOT NULL
);
