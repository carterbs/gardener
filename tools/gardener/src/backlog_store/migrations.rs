use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

use crate::logging::append_run_log;

use super::logging::{db_err, system_time_unix};
use super::types::StoreResult;

pub(super) fn run_migrations(conn: &mut Connection) -> StoreResult<()> {
    let migrations = [
        (1_i64, include_str!("../../migrations/0001_backlog.sql")),
        (2_i64, include_str!("../../migrations/0002_backlog.sql")),
        (3_i64, include_str!("../../migrations/0003_backlog.sql")),
        (4_i64, include_str!("../../migrations/0004_merge_pending.sql")),
        (5_i64, include_str!("../../migrations/0005_rejected_seeds.sql")),
    ];

    conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL); COMMIT;")
        .map_err(db_err)?;

    for (version, sql) in migrations {
        let exists = conn
            .query_row(
                "SELECT 1 FROM schema_migrations WHERE version = ?1 LIMIT 1",
                [version],
                |_| Ok(()),
            )
            .optional()
            .map_err(db_err)?
            .is_some();

        if exists {
            continue;
        }

        append_run_log(
            "info",
            "backlog_store.migration.applying",
            json!({ "version": version }),
        );
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute_batch(sql).map_err(db_err)?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![version, system_time_unix()],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        append_run_log(
            "info",
            "backlog_store.migration.applied",
            json!({ "version": version }),
        );
    }

    Ok(())
}
