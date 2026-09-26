pub mod effects;
pub mod queries;
pub mod schema;
pub mod snapshot_diff;
pub mod state;
pub mod write;

use anyhow::Result;
use rusqlite::Connection;
#[cfg(feature = "native")]
use std::path::{Path, PathBuf};

/// Guard steps are stored as a JSON array so that step text containing any
/// separator character round-trips intact.
pub fn encode_guard_path(guards: &[String]) -> Option<String> {
    if guards.is_empty() {
        None
    } else {
        serde_json::to_string(guards).ok()
    }
}

pub fn decode_guard_path(encoded: Option<&str>) -> Vec<String> {
    encoded
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

/// Get the database file path
/// Tests can override this by setting a different path
#[cfg(feature = "native")]
pub fn get_db_path() -> PathBuf {
    if let Ok(test_db) = std::env::var("SPEC_INDEX_TEST_DB") {
        PathBuf::from(test_db)
    } else {
        // Cross-platform: HOME on Unix, USERPROFILE/known-folder on Windows.
        let home = dirs::home_dir()
            .expect("could not determine home directory (set SPEC_INDEX_TEST_DB to override)");
        home.join(".webspec-index").join("index.db")
    }
}

/// Open or create the database at [`get_db_path`].
#[cfg(feature = "native")]
pub fn open_or_create_db() -> Result<Connection> {
    open_db_at(&get_db_path())
}

/// Open or create the database at `path`, applying schema and migrations.
/// The HTML cache follows the connection into `<path's dir>/html`.
///
/// On first creation (or after `clear-db`), seeds the spec list so that
/// `webspec-index specs` returns all known specs immediately.
#[cfg(feature = "native")]
pub fn open_db_at(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(path)?;
    schema::initialize_schema(&conn)?;
    schema::run_migrations(&conn)?;

    // A build that parses differently invalidates everything derived from the
    // parser. Drop it in one go rather than let specs drift to different parser
    // versions; lazy fetching repopulates whatever gets used.
    schema::purge_if_version_changed(&conn, crate::parse::INDEX_VERSION)?;

    // Seed known specs (W3C, WHATWG, TC39, WebGPU). This is an upsert,
    // so it's safe to call on every open — new specs get added, existing
    // ones are left untouched.
    let _ = crate::spec_list::fetch_and_seed(&conn);

    Ok(conn)
}

/// Refresh the query planner's statistics after indexing writes.
///
/// Without `sqlite_stat1` SQLite picks per-snapshot index scans for joins
/// like the outgoing-calls lookup of `flow`, which are orders of magnitude
/// slower than the plans it chooses with statistics.
pub fn refresh_planner_stats(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA optimize=0x10002;")?;
    Ok(())
}

/// Open an in-memory database with full schema + migrations applied.
/// Used by integration tests and examples that need a ready-to-use connection
/// without a native feature or filesystem path.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    schema::initialize_schema(&conn)?;
    schema::run_migrations(&conn)?;
    Ok(conn)
}

#[cfg(test)]
pub fn open_test_db() -> Result<Connection> {
    open_in_memory()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_planner_stats_analyzes_written_tables() {
        let conn = open_test_db().unwrap();
        let spec = write::insert_or_get_spec(&conn, "DOM", "https://dom.spec.whatwg.org", "whatwg")
            .unwrap();
        write::insert_snapshot(&conn, spec, "hash:d", "2026-01-01T00:00:00Z").unwrap();

        refresh_planner_stats(&conn).unwrap();

        let analyzed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_stat1 WHERE tbl IN ('specs', 'snapshots')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(analyzed > 0);
    }

    #[test]
    fn in_memory_db_initializes_schema_without_native_feature() {
        let conn = open_test_db().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'sections'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
