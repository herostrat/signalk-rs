//! Shared SQLite database for signalk-rs persistence.
//!
//! Provides a single database file with tables for:
//! - **history_raw**: Recent sensor data (configurable retention, default 3 days)
//! - **history_daily**: Aggregated daily summaries (default 90 days)
//! - **track_points**: Vessel position tracks (default 30 days)
//!
//! Uses WAL mode for concurrent reads during writes.

use rusqlite::Connection;
use std::path::Path;
use tracing::info;

pub mod migration;

pub use rusqlite;

/// Shared SQLite database handle.
///
/// Wraps a single `rusqlite::Connection` in WAL mode. The connection is `Send`
/// but not `Sync` — callers should use `Mutex` or `spawn_blocking` for access.
pub struct Database {
    conn: Connection,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Database {
    /// Open (or create) a database at the given path.
    ///
    /// Enables WAL mode and sets a 5-second busy timeout for concurrent access.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::configure(conn)
    }

    /// Open an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure(conn)
    }

    fn configure(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let mut db = Database { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Run schema migrations to bring the database up to date.
    fn migrate(&mut self) -> Result<()> {
        migration::migrate(&self.conn)?;
        Ok(())
    }

    /// Access the underlying connection.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Access the underlying connection mutably.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Consume the `Database` and return the underlying `Connection`.
    ///
    /// Useful when a consumer (e.g. `SqliteTrackStore`) needs to own its own
    /// connection rather than sharing one through references.
    pub fn into_conn(self) -> Connection {
        self.conn
    }

    /// Run VACUUM to reclaim space after large deletions.
    pub fn vacuum(&self) -> Result<()> {
        info!("Running VACUUM on database");
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    /// Open a second read-only connection to the same database file.
    ///
    /// WAL mode allows concurrent reads from multiple connections. Useful for
    /// query-heavy consumers (history API, tracks) that should not block the
    /// ingestion writer.
    pub fn open_reader(path: &Path) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        Ok(conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_and_migrate() {
        let db = Database::open_in_memory().unwrap();
        // Verify schema version was set
        let version: i64 = db
            .conn()
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn wal_mode_enabled() {
        let db = Database::open_in_memory().unwrap();
        let mode: String = db
            .conn()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        // In-memory databases may report "memory" instead of "wal"
        assert!(
            mode == "wal" || mode == "memory",
            "expected wal or memory, got {mode}"
        );
    }

    #[test]
    fn tables_exist_after_migration() {
        let db = Database::open_in_memory().unwrap();
        let tables: Vec<String> = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"history_raw".to_string()));
        assert!(tables.contains(&"history_daily".to_string()));
        assert!(tables.contains(&"track_points".to_string()));
        assert!(tables.contains(&"config".to_string()));
        assert!(tables.contains(&"schema_version".to_string()));
    }

    #[test]
    fn idempotent_migration() {
        let db = Database::open_in_memory().unwrap();
        // Migrate again — should be a no-op
        migration::migrate(db.conn()).unwrap();
        let version: i64 = db
            .conn()
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn vacuum_succeeds() {
        let db = Database::open_in_memory().unwrap();
        db.vacuum().unwrap();
    }

    #[test]
    fn history_raw_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO history_raw (timestamp, context, path, value) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    "2026-03-02T12:00:00Z",
                    "vessels.self",
                    "navigation.speedOverGround",
                    5.14
                ],
            )
            .unwrap();

        let (ts, val): (String, f64) = db
            .conn()
            .query_row(
                "SELECT timestamp, value FROM history_raw WHERE path = ?1",
                ["navigation.speedOverGround"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(ts, "2026-03-02T12:00:00Z");
        assert!((val - 5.14).abs() < 1e-10);
    }

    #[test]
    fn history_daily_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO history_daily (date, context, path, avg_value, min_value, max_value, count) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params!["2026-03-02", "vessels.self", "navigation.speedOverGround", 5.0, 3.0, 7.0, 100],
            )
            .unwrap();

        let (avg, min, max, count): (f64, f64, f64, i64) = db
            .conn()
            .query_row(
                "SELECT avg_value, min_value, max_value, count FROM history_daily WHERE path = ?1",
                ["navigation.speedOverGround"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!((avg - 5.0).abs() < 1e-10);
        assert!((min - 3.0).abs() < 1e-10);
        assert!((max - 7.0).abs() < 1e-10);
        assert_eq!(count, 100);
    }

    #[test]
    fn track_points_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO track_points (context, lat, lon, timestamp, sog, cog, depth) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    "vessels.self",
                    54.321,
                    10.123,
                    "2026-03-02T12:00:00Z",
                    3.5,
                    1.57,
                    12.0
                ],
            )
            .unwrap();

        let (lat, lon): (f64, f64) = db
            .conn()
            .query_row(
                "SELECT lat, lon FROM track_points WHERE context = ?1",
                ["vessels.self"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((lat - 54.321).abs() < 1e-10);
        assert!((lon - 10.123).abs() < 1e-10);
    }

    #[test]
    fn history_and_tracks_share_connection() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();

        // Write history data
        conn.execute(
            "INSERT INTO history_raw (timestamp, context, path, value) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["2026-03-02T12:00:00Z", "vessels.self", "nav.sog", 5.0],
        )
        .unwrap();

        // Write track data on the SAME connection
        conn.execute(
            "INSERT INTO track_points (context, lat, lon, timestamp) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["vessels.self", 54.0, 10.0, "2026-03-02T12:00:00Z"],
        )
        .unwrap();

        // Read both back
        let hist_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_raw", [], |row| row.get(0))
            .unwrap();
        let track_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM track_points", [], |row| row.get(0))
            .unwrap();

        assert_eq!(hist_count, 1);
        assert_eq!(track_count, 1);
    }

    #[test]
    fn concurrent_writes_no_deadlock() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let db = Database::open_in_memory().unwrap();
        let conn = Arc::new(Mutex::new(db.into_conn()));

        let conn_h = conn.clone();
        let conn_t = conn.clone();

        let h1 = thread::spawn(move || {
            for i in 0..100 {
                let c = conn_h.lock().unwrap();
                c.execute(
                    "INSERT INTO history_raw (timestamp, context, path, value) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![format!("2026-03-02T12:00:{i:02}Z"), "vessels.self", "nav.sog", i as f64],
                )
                .unwrap();
            }
        });

        let h2 = thread::spawn(move || {
            for i in 0..100 {
                let c = conn_t.lock().unwrap();
                c.execute(
                    "INSERT INTO track_points (context, lat, lon, timestamp) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params!["vessels.self", 54.0 + i as f64 * 0.001, 10.0, format!("2026-03-02T12:00:{i:02}Z")],
                )
                .unwrap();
            }
        });

        h1.join().unwrap();
        h2.join().unwrap();

        let c = conn.lock().unwrap();
        let hist: i64 = c
            .query_row("SELECT COUNT(*) FROM history_raw", [], |row| row.get(0))
            .unwrap();
        let tracks: i64 = c
            .query_row("SELECT COUNT(*) FROM track_points", [], |row| row.get(0))
            .unwrap();
        assert_eq!(hist, 100);
        assert_eq!(tracks, 100);
    }

    #[test]
    fn file_based_database() {
        let dir = std::env::temp_dir().join("signalk-sqlite-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.db");
        let _ = std::fs::remove_file(&path);

        {
            let db = Database::open(&path).unwrap();
            db.conn()
                .execute(
                    "INSERT INTO history_raw (timestamp, context, path, value) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params!["2026-03-02T12:00:00Z", "vessels.self", "test.path", 42.0],
                )
                .unwrap();
        }

        // Reopen and verify data persists
        {
            let db = Database::open(&path).unwrap();
            let val: f64 = db
                .conn()
                .query_row(
                    "SELECT value FROM history_raw WHERE path = ?1",
                    ["test.path"],
                    |row| row.get(0),
                )
                .unwrap();
            assert!((val - 42.0).abs() < 1e-10);
        }

        let _ = std::fs::remove_file(&path);
    }
}
