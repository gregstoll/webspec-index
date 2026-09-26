//! Snapshot-bound storage shared by all effects consumers.
use anyhow::Result;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};

use crate::effects::engine::IssueId;
use crate::effects::model::SourceSite;

pub fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS effect_structures (
            snapshot_id INTEGER PRIMARY KEY REFERENCES snapshots(id),
            representation_version TEXT NOT NULL,
            structure_json TEXT NOT NULL,
            structure_bytes INTEGER
        );
        CREATE TABLE IF NOT EXISTS effect_anchors (
            snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
            anchor TEXT NOT NULL,
            PRIMARY KEY(snapshot_id, anchor)
        );
        CREATE TABLE IF NOT EXISTS effect_sites (key TEXT PRIMARY KEY, json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS effect_fragments (
            snapshot_id INTEGER PRIMARY KEY REFERENCES snapshots(id),
            spec TEXT NOT NULL,
            indexed_at TEXT NOT NULL,
            config_key TEXT NOT NULL,
            payload BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS effect_publication (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            config_key TEXT NOT NULL, budget_key TEXT NOT NULL, corpus_digest TEXT NOT NULL,
            semantic_key TEXT NOT NULL, manifest_json TEXT NOT NULL,
            opaque_anchor_issue INTEGER, frozen INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS effect_summaries (
            subject_key TEXT PRIMARY KEY, spec TEXT NOT NULL, digest BLOB NOT NULL, payload BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS effect_spec_deps (
            spec TEXT NOT NULL, dep TEXT NOT NULL, edges INTEGER NOT NULL, PRIMARY KEY (spec, dep)
        );",
    )?;
    super::schema::ensure_column(conn, "effect_sites", "spec", "TEXT NOT NULL DEFAULT ''")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_effect_sites_spec ON effect_sites(spec);
        DROP TABLE IF EXISTS effect_graph;
        DROP TABLE IF EXISTS effect_local_matches;
        DROP TABLE IF EXISTS effect_summary_cache;
        DROP TABLE IF EXISTS effect_witnesses;
        DROP TABLE IF EXISTS effect_issues;
        DROP TABLE IF EXISTS effect_subjects;
        DROP TABLE IF EXISTS effect_runs;
        DELETE FROM meta WHERE key = 'effects_generation';",
    )?;
    for table in ["specs", "snapshots", "effect_structures"] {
        for event in ["INSERT", "UPDATE", "DELETE"] {
            conn.execute_batch(&format!(
                "DROP TRIGGER IF EXISTS effects_generation_{table}_{event};"
            ))?;
        }
    }
    let published: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM effect_publication)",
        [],
        |row| row.get(0),
    )?;
    if !published {
        conn.execute("DELETE FROM effect_sites", [])?;
    }
    Ok(())
}

pub fn store_structure(
    conn: &Connection,
    snapshot_id: i64,
    version: &str,
    structure_json: &str,
) -> Result<()> {
    let parsed: serde_json::Value = serde_json::from_str(structure_json)?;
    // Callers normally publish the complete source inside an existing transaction.
    // Joining it also keeps the fragment inventory atomic with the structure.
    super::write::atomic_write(conn, |conn| {
        conn.execute(
            "INSERT INTO effect_structures(snapshot_id, representation_version, structure_json, structure_bytes)
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT(snapshot_id) DO UPDATE SET
             representation_version=excluded.representation_version,
             structure_json=excluded.structure_json, structure_bytes=excluded.structure_bytes",
            (
                snapshot_id,
                version,
                encode_payload_level(structure_json, 1),
                structure_json.len() as i64,
            ),
        )?;
        conn.execute(
            "DELETE FROM effect_anchors WHERE snapshot_id=?1",
            [snapshot_id],
        )?;
        if let Some(anchors) = parsed.get("anchors").and_then(serde_json::Value::as_array) {
            let mut insert = conn.prepare("INSERT OR IGNORE INTO effect_anchors VALUES (?1,?2)")?;
            for anchor in anchors.iter().filter_map(serde_json::Value::as_str) {
                insert.execute((snapshot_id, anchor))?;
            }
        }
        Ok(())
    })
}

pub fn load_structure(
    conn: &Connection,
    snapshot_id: i64,
    version: &str,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT structure_json FROM effect_structures WHERE snapshot_id=?1 AND representation_version=?2",
        (snapshot_id, version),
        |row| Ok(decode_payload(row.get_ref(0)?)),
    )
    .optional()?
    .transpose()
}

/// Raw JSON length of the snapshot's stored structure.
pub fn structure_bytes(conn: &Connection, snapshot_id: i64) -> Result<Option<u64>> {
    let bytes: Option<Option<i64>> = conn
        .query_row(
            "SELECT structure_bytes FROM effect_structures WHERE snapshot_id=?1",
            [snapshot_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(bytes.flatten().map(|bytes| bytes as u64))
}

pub fn has_structure(conn: &Connection, snapshot_id: i64, version: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM effect_structures WHERE snapshot_id=?1 AND representation_version=?2)",
        (snapshot_id, version), |row| row.get(0),
    )?)
}

pub(crate) fn encode_payload(text: &str) -> Vec<u8> {
    encode_payload_level(text, Compression::default().level())
}

pub(crate) fn encode_payload_level(text: &str, level: u32) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(level));
    encoder
        .write_all(text.as_bytes())
        .and_then(|_| encoder.finish())
        .expect("writing to an in-memory buffer cannot fail")
}

pub(crate) fn decode_payload(value: ValueRef<'_>) -> Result<String> {
    match value {
        ValueRef::Text(bytes) => Ok(String::from_utf8(bytes.to_vec())?),
        ValueRef::Blob(bytes) => {
            let mut text = String::new();
            DeflateDecoder::new(bytes).read_to_string(&mut text)?;
            Ok(text)
        }
        other => anyhow::bail!("unexpected payload type {}", other.data_type()),
    }
}

// ----- Fragments, publication and summary rows -----

pub struct SqlSiteStore<'a>(pub &'a Connection);

impl crate::effects::graph::SiteStore for SqlSiteStore<'_> {
    fn site(&self, key: &str) -> Option<SourceSite> {
        // SiteStore is infallible; a missing or unreadable site yields None by
        // design — callers treat absent sites as unknown context.
        self.0
            .query_row("SELECT json FROM effect_sites WHERE key=?1", [key], |row| {
                let json: String = row.get(0)?;
                serde_json::from_str(&json)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
            })
            .optional()
            .ok()
            .flatten()
    }
}

/// Identity of a stored fragment: stale once the snapshot's `indexed_at` or the
/// configuration moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentKey {
    pub snapshot_id: i64,
    pub spec: String,
    pub indexed_at: String,
    pub config_key: String,
}

pub fn fragment_keys(conn: &Connection) -> Result<Vec<FragmentKey>> {
    let mut statement = conn.prepare(
        "SELECT snapshot_id, spec, indexed_at, config_key FROM effect_fragments ORDER BY spec, snapshot_id",
    )?;
    let keys = statement
        .query_map([], |row| {
            Ok(FragmentKey {
                snapshot_id: row.get(0)?,
                spec: row.get(1)?,
                indexed_at: row.get(2)?,
                config_key: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(keys)
}

/// `(spec, payload)` of every fragment built under `config_key`, in spec order.
pub fn load_fragment_payloads(
    conn: &Connection,
    config_key: &str,
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut statement = conn.prepare(
        "SELECT spec, payload FROM effect_fragments WHERE config_key=?1 ORDER BY spec, snapshot_id",
    )?;
    let payloads = statement
        .query_map([config_key], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(payloads)
}

pub fn store_fragment(conn: &Connection, key: &FragmentKey, payload: &[u8]) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO effect_fragments(snapshot_id, spec, indexed_at, config_key, payload)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (key.snapshot_id, &key.spec, &key.indexed_at, &key.config_key, payload),
    )?;
    Ok(())
}

/// Delete the fragments of snapshots that left the corpus.
pub fn delete_orphan_fragments(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM effect_fragments WHERE snapshot_id NOT IN
         (SELECT id FROM snapshots WHERE pr_number IS NULL AND sha LIKE 'hash:%')",
        [],
    )?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub config_key: String,
    pub budget_key: String,
    pub corpus_digest: String,
    pub semantic_key: String,
    pub manifest_json: String,
    pub opaque_anchor_issue: Option<IssueId>,
    /// Exported databases skip the corpus comparison.
    pub frozen: bool,
}

pub fn load_publication(conn: &Connection) -> Result<Option<Publication>> {
    Ok(conn
        .query_row(
            "SELECT config_key, budget_key, corpus_digest, semantic_key, manifest_json,
                    opaque_anchor_issue, frozen
             FROM effect_publication WHERE id=1",
            [],
            |row| {
                Ok(Publication {
                    config_key: row.get(0)?,
                    budget_key: row.get(1)?,
                    corpus_digest: row.get(2)?,
                    semantic_key: row.get(3)?,
                    manifest_json: row.get(4)?,
                    opaque_anchor_issue: row.get::<_, Option<i64>>(5)?.map(|id| id as IssueId),
                    frozen: row.get(6)?,
                })
            },
        )
        .optional()?)
}

pub fn store_publication(conn: &Connection, publication: &Publication) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO effect_publication(id, config_key, budget_key, corpus_digest,
             semantic_key, manifest_json, opaque_anchor_issue, frozen)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            publication.config_key,
            publication.budget_key,
            publication.corpus_digest,
            publication.semantic_key,
            publication.manifest_json,
            publication.opaque_anchor_issue.map(i64::from),
            publication.frozen,
        ],
    )?;
    Ok(())
}

/// Stored digest of every summary row, by subject key.
pub fn summary_digests(conn: &Connection) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut statement = conn.prepare("SELECT subject_key, digest FROM effect_summaries")?;
    let digests = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(digests)
}

/// Upsert `(key, spec, digest, deflated payload)` rows and delete `deletes`.
pub fn apply_summary_diff(
    conn: &Connection,
    upserts: &[(String, String, Vec<u8>, Vec<u8>)],
    deletes: &[String],
) -> Result<()> {
    super::write::atomic_write(conn, |conn| {
        let mut delete = conn.prepare("DELETE FROM effect_summaries WHERE subject_key=?1")?;
        for key in deletes {
            delete.execute([key])?;
        }
        let mut upsert = conn.prepare(
            "INSERT OR REPLACE INTO effect_summaries(subject_key, spec, digest, payload)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (key, spec, digest, payload) in upserts {
            upsert.execute((key, spec, digest, payload))?;
        }
        Ok(())
    })
}

pub fn load_summary(conn: &Connection, subject_key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT payload FROM effect_summaries WHERE subject_key=?1",
        [subject_key],
        |row| Ok(decode_payload(row.get_ref(0)?)),
    )
    .optional()?
    .transpose()
}

/// Replace the site partitions of `specs` with the `sites` that `spec_of` assigns to them.
pub fn replace_site_partitions(
    conn: &Connection,
    specs: &[String],
    sites: &BTreeMap<String, SourceSite>,
    spec_of: impl Fn(&str) -> String,
) -> Result<()> {
    let replaced: BTreeSet<&str> = specs.iter().map(String::as_str).collect();
    super::write::atomic_write(conn, |conn| {
        let mut delete = conn.prepare("DELETE FROM effect_sites WHERE spec=?1")?;
        for spec in &replaced {
            delete.execute([spec])?;
        }
        let mut insert = conn
            .prepare("INSERT OR REPLACE INTO effect_sites(key, json, spec) VALUES (?1, ?2, ?3)")?;
        for (key, site) in sites {
            let spec = spec_of(key);
            if replaced.contains(spec.as_str()) {
                insert.execute((key, serde_json::to_string(site)?, spec))?;
            }
        }
        Ok(())
    })
}

/// Specs that own a site partition.
pub fn site_partition_specs(conn: &Connection) -> Result<Vec<String>> {
    let mut statement = conn.prepare("SELECT DISTINCT spec FROM effect_sites ORDER BY spec")?;
    let specs = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(specs)
}

pub fn store_spec_deps(
    conn: &Connection,
    deps: &BTreeMap<String, BTreeMap<String, u64>>,
) -> Result<()> {
    super::write::atomic_write(conn, |conn| {
        conn.execute("DELETE FROM effect_spec_deps", [])?;
        let mut insert =
            conn.prepare("INSERT INTO effect_spec_deps(spec, dep, edges) VALUES (?1, ?2, ?3)")?;
        for (spec, targets) in deps {
            for (dep, edges) in targets {
                insert.execute((spec, dep, *edges as i64))?;
            }
        }
        Ok(())
    })
}

/// Spec -> `(dependency, edge count)`, both in name order.
pub fn load_spec_deps(conn: &Connection) -> Result<BTreeMap<String, Vec<(String, u64)>>> {
    let mut statement =
        conn.prepare("SELECT spec, dep, edges FROM effect_spec_deps ORDER BY spec, dep")?;
    let mut deps: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (spec, dep, edges) = row?;
        deps.entry(spec).or_default().push((dep, edges as u64));
    }
    Ok(deps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structures_are_stored_deflated_with_their_raw_size() {
        let conn = crate::db::open_test_db().unwrap();
        let spec_id =
            crate::db::write::insert_or_get_spec(&conn, "T", "https://t.test/", "w3c").unwrap();
        let snapshot =
            crate::db::write::insert_snapshot(&conn, spec_id, "hash:t", "2026-09-25").unwrap();
        let json = r#"{"anchors":["a","b"],"algorithms":[]}"#;
        store_structure(&conn, snapshot, "9", json).unwrap();
        let kind: String = conn
            .query_row(
                "SELECT typeof(structure_json) FROM effect_structures",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "blob");
        assert_eq!(
            load_structure(&conn, snapshot, "9").unwrap().as_deref(),
            Some(json)
        );
        assert_eq!(
            structure_bytes(&conn, snapshot).unwrap(),
            Some(json.len() as u64)
        );
        conn.execute("UPDATE effect_structures SET structure_json = ?1", [json])
            .unwrap();
        assert_eq!(
            load_structure(&conn, snapshot, "9").unwrap().as_deref(),
            Some(json),
            "legacy TEXT rows still load"
        );
    }

    #[test]
    fn payload_round_trips_through_deflate() {
        let text = r#"{"hops":[{"from":{"spec":"HTML","anchor":"navigate"}}]}"#;
        let encoded = encode_payload(text);
        assert!(encoded.len() < text.len() * 2);
        let decoded = decode_payload(rusqlite::types::ValueRef::Blob(&encoded)).unwrap();
        assert_eq!(decoded, text);
    }

    #[test]
    fn legacy_text_payload_is_returned_verbatim() {
        let decoded =
            decode_payload(rusqlite::types::ValueRef::Text(b"{\"legacy\":true}")).unwrap();
        assert_eq!(decoded, "{\"legacy\":true}");
    }

    fn snapshot(conn: &Connection, spec: &str) -> i64 {
        let spec_id =
            crate::db::write::insert_or_get_spec(conn, spec, "https://t.test/", "w3c").unwrap();
        crate::db::write::insert_snapshot(conn, spec_id, "hash:t", "2026-09-25").unwrap()
    }

    fn key(snapshot_id: i64, spec: &str, config_key: &str) -> FragmentKey {
        FragmentKey {
            snapshot_id,
            spec: spec.into(),
            indexed_at: "t0".into(),
            config_key: config_key.into(),
        }
    }

    #[test]
    fn fragments_round_trip_by_config_key_in_spec_order() {
        let conn = crate::db::open_test_db().unwrap();
        let (b, a) = (snapshot(&conn, "B"), snapshot(&conn, "A"));
        store_fragment(&conn, &key(b, "B", "c1"), b"bee").unwrap();
        store_fragment(&conn, &key(a, "A", "c1"), b"ay").unwrap();
        assert_eq!(
            load_fragment_payloads(&conn, "c1").unwrap(),
            [
                ("A".to_string(), b"ay".to_vec()),
                ("B".into(), b"bee".to_vec())
            ]
        );
        assert!(load_fragment_payloads(&conn, "c2").unwrap().is_empty());
        store_fragment(&conn, &key(a, "A", "c2"), b"ay2").unwrap();
        let keys = fragment_keys(&conn).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(
            (keys[0].spec.as_str(), keys[0].config_key.as_str()),
            ("A", "c2")
        );
        assert_eq!(delete_orphan_fragments(&conn).unwrap(), 0);
        conn.execute("UPDATE snapshots SET sha='other' WHERE id=?1", [b])
            .unwrap();
        assert_eq!(delete_orphan_fragments(&conn).unwrap(), 1);
    }

    #[test]
    fn summary_diff_upserts_and_deletes() {
        let conn = crate::db::open_test_db().unwrap();
        let row = |k: &str, d: &[u8], p: &str| {
            (
                k.to_string(),
                "S".to_string(),
                d.to_vec(),
                encode_payload(p),
            )
        };
        apply_summary_diff(
            &conn,
            &[row("a", b"1", "{\"a\":1}"), row("b", b"2", "{}")],
            &[],
        )
        .unwrap();
        apply_summary_diff(&conn, &[row("a", b"3", "{\"a\":3}")], &["b".into()]).unwrap();
        assert_eq!(
            summary_digests(&conn).unwrap(),
            BTreeMap::from([("a".to_string(), b"3".to_vec())])
        );
        assert_eq!(
            load_summary(&conn, "a").unwrap().as_deref(),
            Some("{\"a\":3}")
        );
        assert_eq!(load_summary(&conn, "b").unwrap(), None);
    }

    #[test]
    fn publication_and_spec_deps_round_trip() {
        let conn = crate::db::open_test_db().unwrap();
        assert!(load_publication(&conn).unwrap().is_none());
        let publication = Publication {
            config_key: "c".into(),
            budget_key: "b".into(),
            corpus_digest: "d".into(),
            semantic_key: "s".into(),
            manifest_json: "{}".into(),
            opaque_anchor_issue: Some(4),
            frozen: false,
        };
        store_publication(&conn, &publication).unwrap();
        assert_eq!(load_publication(&conn).unwrap(), Some(publication));
        let deps = BTreeMap::from([(
            "DOM".to_string(),
            BTreeMap::from([("INFRA".to_string(), 3u64), ("URL".into(), 1)]),
        )]);
        store_spec_deps(&conn, &deps).unwrap();
        assert_eq!(
            load_spec_deps(&conn).unwrap(),
            BTreeMap::from([(
                "DOM".to_string(),
                vec![("INFRA".to_string(), 3), ("URL".into(), 1)]
            )])
        );
    }

    #[test]
    fn site_partitions_are_replaced_per_spec() {
        use crate::effects::graph::SiteStore as _;
        let conn = crate::db::open_test_db().unwrap();
        let site = |id: &str| SourceSite {
            id: id.into(),
            subject: crate::effects::model::Subject {
                spec: "S".into(),
                anchor: id.into(),
                snapshot_sha: "hash:s".into(),
                step_id: None,
                step_path: None,
                body_id: None,
            },
            url: format!("https://t.test/#{id}"),
            segment_id: None,
            span: None,
            step_text: None,
        };
        let spec_of = |key: &str| key.split(':').next().unwrap().to_string();
        let sites = BTreeMap::from([("A:1".to_string(), site("1")), ("B:2".into(), site("2"))]);
        replace_site_partitions(&conn, &["A".into(), "B".into()], &sites, spec_of).unwrap();
        let sites = BTreeMap::from([("A:3".to_string(), site("3")), ("B:4".into(), site("4"))]);
        replace_site_partitions(&conn, &["A".into()], &sites, spec_of).unwrap();
        let store = SqlSiteStore(&conn);
        assert!(store.site("A:1").is_none());
        assert_eq!(store.site("A:3"), Some(site("3")));
        assert_eq!(store.site("B:2"), Some(site("2")));
        assert!(store.site("B:4").is_none(), "B was not replaced");
    }

    #[test]
    fn initialize_drops_the_generation_machinery() {
        let conn = crate::db::open_test_db().unwrap();
        conn.execute_batch(
            "CREATE TABLE effect_graph (id INTEGER);
             CREATE TRIGGER effects_generation_specs_INSERT AFTER INSERT ON specs BEGIN SELECT 1; END;
             INSERT INTO meta(key, value) VALUES ('effects_generation', '3');
             INSERT INTO effect_sites(key, json, spec) VALUES ('k', '{}', 'S');",
        )
        .unwrap();
        initialize(&conn).unwrap();
        let leftovers: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sqlite_master WHERE name IN ('effect_graph','effects_generation_specs_INSERT'))
                      + (SELECT COUNT(*) FROM meta WHERE key='effects_generation')
                      + (SELECT COUNT(*) FROM effect_sites)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leftovers, 0);
    }
}
