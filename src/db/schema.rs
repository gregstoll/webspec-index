use anyhow::Result;
use rusqlite::Connection;

pub fn initialize_schema(conn: &Connection) -> Result<()> {
    // Check if already initialized
    let table_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='specs'",
        [],
        |row| row.get(0),
    )?;

    if table_count > 0 {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE specs (
            id          INTEGER PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            base_url    TEXT NOT NULL,
            provider    TEXT NOT NULL
        );

        CREATE TABLE snapshots (
            id          INTEGER PRIMARY KEY,
            spec_id     INTEGER NOT NULL REFERENCES specs(id),
            sha         TEXT NOT NULL,
            commit_date TEXT NOT NULL,
            indexed_at  TEXT NOT NULL,
            is_latest   INTEGER NOT NULL DEFAULT 0,
            pr_number   INTEGER,
            merge_base_sha TEXT,
            pr_pages    TEXT,
            index_version TEXT,
            UNIQUE(spec_id, sha)
        );

        CREATE TABLE sections (
            id            INTEGER PRIMARY KEY,
            snapshot_id   INTEGER NOT NULL REFERENCES snapshots(id),
            anchor        TEXT NOT NULL,
            title         TEXT,
            content_text  TEXT,
            section_type  TEXT NOT NULL,
            parent_anchor TEXT,
            prev_anchor   TEXT,
            next_anchor   TEXT,
            depth         INTEGER,
            number        TEXT,
            UNIQUE(snapshot_id, anchor)
        );

        CREATE INDEX idx_sections_parent ON sections(snapshot_id, parent_anchor);

        CREATE TABLE refs (
            id           INTEGER PRIMARY KEY,
            snapshot_id  INTEGER NOT NULL REFERENCES snapshots(id),
            from_anchor  TEXT NOT NULL,
            to_spec      TEXT NOT NULL,
            to_anchor    TEXT NOT NULL,
            step_path    TEXT,
            step_text    TEXT,
            guard_path   TEXT,
            call_site_id TEXT,
            kind         TEXT
        );

        CREATE INDEX idx_refs_outgoing ON refs(snapshot_id, from_anchor);
        CREATE INDEX idx_refs_incoming ON refs(snapshot_id, to_spec, to_anchor);
        CREATE INDEX idx_refs_to ON refs(to_spec, to_anchor);

        CREATE TABLE idl_defs (
            id             INTEGER PRIMARY KEY,
            snapshot_id    INTEGER NOT NULL REFERENCES snapshots(id),
            anchor         TEXT NOT NULL,
            name           TEXT NOT NULL,
            owner          TEXT,
            kind           TEXT NOT NULL,
            canonical_name TEXT NOT NULL,
            idl_text       TEXT,
            UNIQUE(snapshot_id, anchor, kind)
        );

        CREATE INDEX idx_idl_defs_anchor ON idl_defs(snapshot_id, anchor);
        CREATE INDEX idx_idl_defs_canonical ON idl_defs(snapshot_id, canonical_name);

        CREATE TABLE update_checks (
            spec_id        INTEGER PRIMARY KEY REFERENCES specs(id),
            last_checked   TEXT NOT NULL,
            last_indexed   TEXT,
            content_hash   TEXT,
            index_version TEXT
        );

        CREATE TABLE meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE VIRTUAL TABLE sections_fts USING fts5(
            anchor,
            title,
            content_text,
            content=sections,
            content_rowid=id
        );

        CREATE TRIGGER sections_ai AFTER INSERT ON sections BEGIN
            INSERT INTO sections_fts(rowid, anchor, title, content_text)
            VALUES (new.id, new.anchor, new.title, new.content_text);
        END;

        CREATE TRIGGER sections_ad AFTER DELETE ON sections BEGIN
            INSERT INTO sections_fts(sections_fts, rowid, anchor, title, content_text)
            VALUES ('delete', old.id, old.anchor, old.title, old.content_text);
        END;

        CREATE TRIGGER sections_au AFTER UPDATE ON sections BEGIN
            INSERT INTO sections_fts(sections_fts, rowid, anchor, title, content_text)
            VALUES ('delete', old.id, old.anchor, old.title, old.content_text);
            INSERT INTO sections_fts(rowid, anchor, title, content_text)
            VALUES (new.id, new.anchor, new.title, new.content_text);
        END;
        "#,
    )?;

    Ok(())
}

/// Run schema migrations for tables added after initial release.
/// Uses CREATE TABLE IF NOT EXISTS to be safe on both new and existing databases.
pub fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS idl_defs (
            id             INTEGER PRIMARY KEY,
            snapshot_id    INTEGER NOT NULL REFERENCES snapshots(id),
            anchor         TEXT NOT NULL,
            name           TEXT NOT NULL,
            owner          TEXT,
            kind           TEXT NOT NULL,
            canonical_name TEXT NOT NULL,
            idl_text       TEXT,
            UNIQUE(snapshot_id, anchor, kind)
        );
        CREATE INDEX IF NOT EXISTS idx_idl_defs_anchor ON idl_defs(snapshot_id, anchor);
        CREATE INDEX IF NOT EXISTS idx_idl_defs_canonical ON idl_defs(snapshot_id, canonical_name);
        CREATE TABLE IF NOT EXISTS update_checks (
            spec_id        INTEGER PRIMARY KEY REFERENCES specs(id),
            last_checked   TEXT NOT NULL,
            last_indexed   TEXT,
            content_hash   TEXT,
            index_version TEXT
        );
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;

    if has_table(conn, "refs")? {
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_refs_to ON refs(to_spec, to_anchor);")?;
    }

    // Rename the pre-existing `parser_version` column (from an earlier build)
    // to `index_version`, preserving its data, before ensuring the column exists.
    rename_column(conn, "update_checks", "parser_version", "index_version")?;
    rename_column(conn, "snapshots", "parser_version", "index_version")?;

    ensure_column(conn, "update_checks", "last_indexed", "TEXT")?;
    ensure_column(conn, "update_checks", "content_hash", "TEXT")?;
    ensure_column(conn, "update_checks", "index_version", "TEXT")?;
    ensure_column(conn, "snapshots", "pr_number", "INTEGER")?;
    ensure_column(conn, "snapshots", "merge_base_sha", "TEXT")?;
    ensure_column(conn, "snapshots", "pr_pages", "TEXT")?;
    ensure_column(conn, "snapshots", "index_version", "TEXT")?;
    // Step-level reference context. Rows written before this landed keep NULLs;
    // the index version bump forces a re-parse that fills them in.
    ensure_column(conn, "refs", "step_path", "TEXT")?;
    ensure_column(conn, "refs", "step_text", "TEXT")?;
    ensure_column(conn, "refs", "guard_path", "TEXT")?;
    ensure_column(conn, "refs", "call_site_id", "TEXT")?;
    ensure_column(conn, "refs", "kind", "TEXT")?;
    ensure_column(conn, "sections", "number", "TEXT")?;
    super::effects::initialize(conn)?;
    super::state::initialize(conn)?;
    Ok(())
}

/// Indexed data is derived from the parser, so a build that parses differently
/// invalidates all of it. Rather than let specs drift to different parser
/// versions as each is lazily re-checked, drop everything derived on the first
/// run of a new build and let the normal lazy fetch repopulate what is used.
///
/// The spec registry itself is kept: it is a list of names and URLs, not parsed
/// output, and re-seeding it costs a network round trip for no benefit.
///
/// Returns whether a purge happened.
pub fn purge_if_version_changed(conn: &Connection, current: &str) -> Result<bool> {
    if indexed_version(conn)?.as_deref() == Some(current) {
        return Ok(false);
    }

    let tx = conn.unchecked_transaction()?;
    // Order matters: children before the snapshots they reference.
    for table in [
        "state_coverage",
        "state_occurrence_counts",
        "state_sites",
        "state_members",
        "state_field_owners",
        "state_fields",
        "state_type_edges",
        "state_types",
        "state_models",
        "effect_sites",
        "effect_graph",
        "effect_local_matches",
        "effect_anchors",
        "effect_structures",
        "refs",
        "idl_defs",
        "sections",
        "snapshots",
        "update_checks",
    ] {
        if has_table(&tx, table)? {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
    }
    tx.commit()?;

    // DELETE frees pages without returning them to the filesystem, so without
    // this the purge is only logical and the file keeps its old size. VACUUM
    // cannot run inside a transaction, hence after the commit. Measured at ~1s
    // on a 430 MB index.
    conn.execute_batch("VACUUM")?;

    set_indexed_version(conn, current)?;
    Ok(true)
}

/// The build that produced the currently indexed data, if recorded.
pub fn indexed_version(conn: &Connection) -> Result<Option<String>> {
    if !has_table(conn, "meta")? {
        return Ok(None);
    }
    let result = conn.query_row(
        "SELECT value FROM meta WHERE key = 'index_version'",
        [],
        |row| row.get(0),
    );
    match result {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn set_indexed_version(conn: &Connection, version: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('index_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [version],
    )?;
    Ok(())
}

fn has_table(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_column(conn: &Connection, table: &str, column: &str, kind: &str) -> Result<()> {
    if !has_table(conn, table)? {
        return Ok(());
    }
    if !has_column(conn, table, column)? {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"),
            [],
        )?;
    }
    Ok(())
}

/// Rename a column, but only when the old name still exists and the new name
/// does not — so the migration is a no-op on already-migrated and fresh databases.
fn rename_column(conn: &Connection, table: &str, old: &str, new: &str) -> Result<()> {
    if has_column(conn, table, old)? && !has_column(conn, table, new)? {
        conn.execute(
            &format!("ALTER TABLE {table} RENAME COLUMN {old} TO {new}"),
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_initialization() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"specs".to_string()));
        assert!(tables.contains(&"snapshots".to_string()));
        assert!(tables.contains(&"sections".to_string()));
        assert!(tables.contains(&"refs".to_string()));
        assert!(tables.contains(&"idl_defs".to_string()));
        assert!(tables.contains(&"update_checks".to_string()));
    }

    #[test]
    fn test_schema_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        // Should not fail on second call
        initialize_schema(&conn).unwrap();
    }

    #[test]
    fn test_migrations() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"idl_defs".to_string()));
    }

    #[test]
    fn test_migrations_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        run_migrations(&conn).unwrap();
        // Should not fail on second call
        run_migrations(&conn).unwrap();
    }

    #[test]
    fn test_pr_columns_migration() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        run_migrations(&conn).unwrap();

        // Verify pr_number column exists and is nullable
        conn.execute(
            "INSERT INTO specs (name, base_url, provider) VALUES ('TEST', 'https://test', 'test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO snapshots (spec_id, sha, commit_date, indexed_at, pr_number, merge_base_sha)
             VALUES (1, 'abc', '2026-01-01', '2026-01-01', 12345, 'def456')",
            [],
        ).unwrap();

        let (pr, base): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT pr_number, merge_base_sha FROM snapshots WHERE sha = 'abc'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(pr, Some(12345));
        assert_eq!(base.as_deref(), Some("def456"));

        // Verify trunk snapshots have NULL pr_number
        conn.execute(
            "INSERT INTO snapshots (spec_id, sha, commit_date, indexed_at)
             VALUES (1, 'xyz', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        let pr: Option<i64> = conn
            .query_row(
                "SELECT pr_number FROM snapshots WHERE sha = 'xyz'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pr, None);
    }

    #[test]
    fn test_index_version_column_migration() {
        // Simulate a pre-parser-version database: update_checks without the
        // column, containing a row written by an older binary.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE specs (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                base_url TEXT NOT NULL,
                provider TEXT NOT NULL
            );
            CREATE TABLE snapshots (
                id          INTEGER PRIMARY KEY,
                spec_id     INTEGER NOT NULL REFERENCES specs(id),
                sha         TEXT NOT NULL,
                commit_date TEXT NOT NULL,
                indexed_at  TEXT NOT NULL,
                is_latest   INTEGER NOT NULL DEFAULT 0,
                UNIQUE(spec_id, sha)
            );
            CREATE TABLE update_checks (
                spec_id      INTEGER PRIMARY KEY REFERENCES specs(id),
                last_checked TEXT NOT NULL,
                last_indexed TEXT,
                content_hash TEXT
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO specs (name, base_url, provider) VALUES ('TEST', 'https://test', 'test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO update_checks (spec_id, last_checked, content_hash)
             VALUES (1, '2026-01-01T00:00:00Z', 'oldhash')",
            [],
        )
        .unwrap();
        // A snapshot written by an older binary (no index_version column yet).
        conn.execute(
            "INSERT INTO snapshots (spec_id, sha, commit_date, indexed_at)
             VALUES (1, 'oldsha', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        // Migration must add the columns without dropping the existing rows.
        run_migrations(&conn).unwrap();

        // Rows written before the upgrade read back as NULL index_version,
        // which is what forces a re-parse on the next sync.
        let pv: Option<String> = conn
            .query_row(
                "SELECT index_version FROM update_checks WHERE spec_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pv, None);
        let snap_pv: Option<String> = conn
            .query_row(
                "SELECT index_version FROM snapshots WHERE sha = 'oldsha'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(snap_pv, None, "snapshots.index_version should migrate too");

        // The column is writable after migration.
        conn.execute(
            "UPDATE update_checks SET index_version = '0.5.0' WHERE spec_id = 1",
            [],
        )
        .unwrap();
        let pv: Option<String> = conn
            .query_row(
                "SELECT index_version FROM update_checks WHERE spec_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pv.as_deref(), Some("0.5.0"));
    }

    #[test]
    fn test_parser_version_renamed_to_index_version() {
        // Simulate a database from the intermediate build that still has the
        // old `parser_version` column, with a value that must be preserved.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE specs (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                base_url TEXT NOT NULL,
                provider TEXT NOT NULL
            );
            CREATE TABLE snapshots (
                id          INTEGER PRIMARY KEY,
                spec_id     INTEGER NOT NULL REFERENCES specs(id),
                sha         TEXT NOT NULL,
                commit_date TEXT NOT NULL,
                indexed_at  TEXT NOT NULL,
                parser_version TEXT,
                UNIQUE(spec_id, sha)
            );
            CREATE TABLE update_checks (
                spec_id        INTEGER PRIMARY KEY REFERENCES specs(id),
                last_checked   TEXT NOT NULL,
                last_indexed   TEXT,
                content_hash   TEXT,
                parser_version TEXT
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO specs (name, base_url, provider) VALUES ('TEST', 'https://test', 'test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO update_checks (spec_id, last_checked, content_hash, parser_version)
             VALUES (1, '2026-01-01T00:00:00Z', 'oldhash', '0.11.0')",
            [],
        )
        .unwrap();

        run_migrations(&conn).unwrap();

        // The old column is gone and its value survived under the new name.
        assert!(!has_column(&conn, "update_checks", "parser_version").unwrap());
        assert!(has_column(&conn, "update_checks", "index_version").unwrap());
        assert!(has_column(&conn, "snapshots", "index_version").unwrap());
        let pv: Option<String> = conn
            .query_row(
                "SELECT index_version FROM update_checks WHERE spec_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pv.as_deref(), Some("0.11.0"));

        // Re-running is a no-op (rename guard sees the new column already exists).
        run_migrations(&conn).unwrap();
    }

    #[test]
    fn test_pr_pages_column_migration() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        run_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO specs (name, base_url, provider) VALUES ('TEST', 'https://test', 'test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO snapshots (spec_id, sha, commit_date, indexed_at, pr_number, merge_base_sha, pr_pages)
             VALUES (1, 'abc', '2026-01-01', '2026-01-01', 123, 'def', 'page1.html,page2.html')",
            [],
        ).unwrap();

        let pages: Option<String> = conn
            .query_row(
                "SELECT pr_pages FROM snapshots WHERE sha = 'abc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pages.as_deref(), Some("page1.html,page2.html"));
    }

    fn indexed_row_counts(conn: &Connection) -> (i64, i64, i64, i64, i64) {
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        (
            count("specs"),
            count("snapshots"),
            count("sections"),
            count("refs"),
            count("update_checks"),
        )
    }

    fn seed_indexed_data(conn: &Connection) {
        conn.execute(
            "INSERT INTO specs (name, base_url, provider) VALUES ('HTML', 'https://html', 'whatwg')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO snapshots (spec_id, sha, commit_date, indexed_at)
             VALUES (1, 'hash:abc', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sections (snapshot_id, anchor, section_type) VALUES (1, 'navigate', 'algorithm')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO refs (snapshot_id, from_anchor, to_spec, to_anchor)
             VALUES (1, 'navigate', 'HTML', 'checking')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO update_checks (spec_id, last_checked) VALUES (1, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn test_purge_on_version_change_clears_indexed_data_but_keeps_spec_list() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        run_migrations(&conn).unwrap();
        seed_indexed_data(&conn);
        set_indexed_version(&conn, "0.12.0").unwrap();

        let purged = purge_if_version_changed(&conn, "0.13.0").unwrap();

        assert!(purged, "a version change must purge");
        // The purge VACUUMs, which fails if a transaction is still open; if that
        // regressed this call would have errored above rather than returned.
        let (specs, snapshots, sections, refs, checks) = indexed_row_counts(&conn);
        assert_eq!(
            specs, 1,
            "the spec list is a registry, not parsed output, and must survive"
        );
        assert_eq!((snapshots, sections, refs, checks), (0, 0, 0, 0));
        assert_eq!(indexed_version(&conn).unwrap().as_deref(), Some("0.13.0"));
    }

    #[test]
    fn test_no_purge_when_version_matches() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        run_migrations(&conn).unwrap();
        seed_indexed_data(&conn);
        set_indexed_version(&conn, "0.13.0").unwrap();

        let purged = purge_if_version_changed(&conn, "0.13.0").unwrap();

        assert!(!purged);
        let (_, snapshots, sections, refs, checks) = indexed_row_counts(&conn);
        assert_eq!((snapshots, sections, refs, checks), (1, 1, 1, 1));
    }

    #[test]
    fn test_database_predating_the_version_marker_is_purged() {
        // Every database written before this landed has indexed data and no
        // marker, and that data was produced by an older parser.
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        run_migrations(&conn).unwrap();
        seed_indexed_data(&conn);

        assert!(indexed_version(&conn).unwrap().is_none());
        assert!(purge_if_version_changed(&conn, "0.13.0").unwrap());

        let (specs, snapshots, _, _, _) = indexed_row_counts(&conn);
        assert_eq!((specs, snapshots), (1, 0));
    }

    #[test]
    fn test_purge_is_idempotent_and_cheap_on_a_fresh_database() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        run_migrations(&conn).unwrap();

        // A fresh database has nothing to purge, but still records the version so
        // the next upgrade is detectable.
        assert!(purge_if_version_changed(&conn, "0.13.0").unwrap());
        assert!(!purge_if_version_changed(&conn, "0.13.0").unwrap());
        assert_eq!(indexed_version(&conn).unwrap().as_deref(), Some("0.13.0"));
    }

    #[test]
    fn test_refs_step_columns_migration() {
        // A database written before step-level references existed.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE specs (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                base_url TEXT NOT NULL,
                provider TEXT NOT NULL
            );
            CREATE TABLE snapshots (
                id          INTEGER PRIMARY KEY,
                spec_id     INTEGER NOT NULL REFERENCES specs(id),
                sha         TEXT NOT NULL,
                commit_date TEXT NOT NULL,
                indexed_at  TEXT NOT NULL,
                is_latest   INTEGER NOT NULL DEFAULT 0,
                UNIQUE(spec_id, sha)
            );
            CREATE TABLE refs (
                id           INTEGER PRIMARY KEY,
                snapshot_id  INTEGER NOT NULL REFERENCES snapshots(id),
                from_anchor  TEXT NOT NULL,
                to_spec      TEXT NOT NULL,
                to_anchor    TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO specs (name, base_url, provider) VALUES ('TEST', 'https://test', 'test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO snapshots (spec_id, sha, commit_date, indexed_at)
             VALUES (1, 'oldsha', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO refs (snapshot_id, from_anchor, to_spec, to_anchor)
             VALUES (1, 'navigate', 'TEST', 'target')",
            [],
        )
        .unwrap();

        run_migrations(&conn).unwrap();

        // The pre-existing row survives, with NULL step context.
        let (step_path, kind): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT step_path, kind FROM refs WHERE from_anchor = 'navigate'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(step_path, None);
        assert_eq!(kind, None);

        // And the new columns are writable.
        conn.execute(
            "UPDATE refs SET step_path = '24.1', step_text = 'Let x be y.',
                             guard_path = 'If cond:', call_site_id = 'navigate:target-3',
                             kind = 'step'
             WHERE from_anchor = 'navigate'",
            [],
        )
        .unwrap();

        run_migrations(&conn).unwrap();
    }

    #[test]
    fn test_fresh_schema_has_refs_step_columns() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(refs)").unwrap();
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            rows
        };

        for expected in [
            "step_path",
            "step_text",
            "guard_path",
            "call_site_id",
            "kind",
        ] {
            assert!(
                columns.contains(&expected.to_string()),
                "fresh schema is missing refs.{expected}"
            );
        }
    }

    #[test]
    fn incoming_ref_lookup_uses_target_index() {
        let conn = crate::db::open_test_db().unwrap();
        let plan: Vec<String> = conn
            .prepare("EXPLAIN QUERY PLAN SELECT from_anchor FROM refs WHERE to_spec = 'HTML' AND to_anchor = 'navigate'")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter().any(|line| line.contains("idx_refs_to")),
            "{plan:?}"
        );
    }

    #[test]
    fn migrations_add_target_index_to_existing_databases() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        conn.execute_batch("DROP INDEX idx_refs_to;").unwrap();
        run_migrations(&conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_refs_to'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
