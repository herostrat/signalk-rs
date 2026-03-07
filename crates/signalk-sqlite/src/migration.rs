//! Schema migration for the signalk-rs SQLite database.
//!
//! Each schema version is applied exactly once within its own transaction.
//! If a migration fails partway through, the transaction rolls back and
//! the database remains at the previous version (no half-applied state).
//!
//! The `schema_version` table tracks which versions have been applied.

use rusqlite::Connection;
use tracing::info;

/// The latest schema version this build knows how to apply.
pub const CURRENT_VERSION: i64 = 1;

/// Run all pending migrations.
pub fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Ensure the version table exists
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);")?;

    let current = current_version(conn)?;

    if current < 1 {
        migrate_v1(conn)?;
    }
    // Future: if current < 2 { migrate_v2(conn)?; }

    Ok(())
}

/// Query the current schema version (0 if no migrations applied yet).
pub fn current_version(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )
}

/// Schema version 1: initial tables for history, tracks.
///
/// Wrapped in a transaction so that a failure leaves the DB clean.
fn migrate_v1(conn: &Connection) -> Result<(), rusqlite::Error> {
    info!("Applying database migration v1");

    let tx = conn.unchecked_transaction()?;

    tx.execute_batch(
        "
        -- Raw sensor data (short-term retention)
        CREATE TABLE IF NOT EXISTS history_raw (
            timestamp TEXT NOT NULL,
            context   TEXT NOT NULL,
            path      TEXT NOT NULL,
            value     REAL,
            value_str TEXT,
            source    TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_raw_ctx_path_ts
            ON history_raw(context, path, timestamp);

        -- Daily aggregates (long-term retention)
        CREATE TABLE IF NOT EXISTS history_daily (
            date      TEXT NOT NULL,
            context   TEXT NOT NULL,
            path      TEXT NOT NULL,
            avg_value REAL,
            min_value REAL,
            max_value REAL,
            count     INTEGER,
            PRIMARY KEY (date, context, path)
        );

        -- Track points (vessel position history)
        CREATE TABLE IF NOT EXISTS track_points (
            id        INTEGER PRIMARY KEY,
            context   TEXT NOT NULL,
            lat       REAL NOT NULL,
            lon       REAL NOT NULL,
            timestamp TEXT NOT NULL,
            sog       REAL,
            cog       REAL,
            depth     REAL
        );
        CREATE INDEX IF NOT EXISTS idx_track_ctx_ts
            ON track_points(context, timestamp);

        -- Runtime-mutable configuration (vessel, plugins, priorities, TTLs)
        CREATE TABLE IF NOT EXISTS config (
            namespace  TEXT NOT NULL,
            key        TEXT NOT NULL,
            value      TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (namespace, key)
        );

        -- Record migration
        INSERT INTO schema_version (version) VALUES (1);
        ",
    )?;

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_fresh_database() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 1);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 1);
    }

    #[test]
    fn current_version_empty_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);",
        )
        .unwrap();
        assert_eq!(current_version(&conn).unwrap(), 0);
    }

    #[test]
    fn schema_version_matches_constant() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn data_survives_re_migration() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        // Insert some data
        conn.execute(
            "INSERT INTO history_raw (timestamp, context, path, value) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["2026-03-02T12:00:00Z", "vessels.self", "nav.sog", 5.0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO track_points (context, lat, lon, timestamp) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["vessels.self", 54.0, 10.0, "2026-03-02T12:00:00Z"],
        )
        .unwrap();

        // Run migrate again (should be no-op)
        migrate(&conn).unwrap();

        // Verify data survived
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_raw", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "history_raw data must survive re-migration");

        let track_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM track_points", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            track_count, 1,
            "track_points data must survive re-migration"
        );
    }

    #[test]
    fn current_version_is_accessible() {
        // This test verifies that current_version is pub and callable externally
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);",
        )
        .unwrap();
        let v = current_version(&conn).unwrap();
        assert_eq!(v, 0);
    }

    #[test]
    fn migration_transaction_protects_version() {
        // Simulate: if v1 has been applied, a second apply is a no-op (no double insert)
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 1);

        // Manually try to insert version 1 again — should fail (PK constraint)
        let result = conn.execute("INSERT INTO schema_version (version) VALUES (1)", []);
        assert!(result.is_err(), "double version insert must fail");

        // Version should still be 1
        assert_eq!(current_version(&conn).unwrap(), 1);
    }
}
