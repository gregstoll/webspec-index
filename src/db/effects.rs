//! Snapshot-bound storage shared by all effects consumers.
use anyhow::Result;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS effect_structures (
            snapshot_id INTEGER PRIMARY KEY REFERENCES snapshots(id),
            representation_version TEXT NOT NULL,
            structure_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS effect_anchors (
            snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
            anchor TEXT NOT NULL,
            PRIMARY KEY(snapshot_id, anchor)
        );
        CREATE TABLE IF NOT EXISTS effect_local_matches (
            input_key TEXT PRIMARY KEY, payload_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS effect_runs (
            analysis_id TEXT PRIMARY KEY,
            generation INTEGER NOT NULL,
            semantic_key TEXT NOT NULL,
            scope_key TEXT NOT NULL,
            budget_key TEXT NOT NULL,
            reached_fixed_point INTEGER NOT NULL,
            manifest_json TEXT NOT NULL,
            artifact_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS effect_runs_lookup ON effect_runs(semantic_key, generation);
        CREATE TABLE IF NOT EXISTS effect_subjects (
            analysis_id TEXT NOT NULL REFERENCES effect_runs(analysis_id),
            subject_key TEXT NOT NULL,
            summary_json TEXT NOT NULL,
            PRIMARY KEY(analysis_id, subject_key)
        );
        CREATE TABLE IF NOT EXISTS effect_witnesses (
            analysis_id TEXT NOT NULL REFERENCES effect_runs(analysis_id),
            subject_key TEXT NOT NULL,
            effect_id TEXT NOT NULL,
            witness_json TEXT NOT NULL,
            PRIMARY KEY(analysis_id, subject_key, effect_id)
        );
        CREATE TABLE IF NOT EXISTS effect_issues (
            analysis_id TEXT NOT NULL REFERENCES effect_runs(analysis_id),
            issue_id TEXT NOT NULL,
            issue_json TEXT NOT NULL,
            PRIMARY KEY(analysis_id, issue_id)
        );
        INSERT OR IGNORE INTO meta(key, value) VALUES ('effects_generation', '0');
        INSERT OR IGNORE INTO meta(key, value) VALUES ('effects_publication', '0');",
    )?;
    // Publishing summaries changes reader caches, not the semantic input key.
    for event in ["INSERT", "UPDATE", "DELETE"] {
        conn.execute_batch(&format!(
            "CREATE TRIGGER IF NOT EXISTS effects_publication_{event}
             AFTER {event} ON effect_runs BEGIN
             UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'effects_publication';
             END;"
        ))?;
    }
    // These triggers also cover mutation paths outside the effects engine.
    // Triggers participate in the writer's transaction, including rollbacks.
    for table in ["specs", "snapshots", "effect_structures"] {
        for event in ["INSERT", "UPDATE", "DELETE"] {
            conn.execute_batch(&format!(
                "CREATE TRIGGER IF NOT EXISTS effects_generation_{table}_{event}
                 AFTER {event} ON {table} BEGIN
                 UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'effects_generation';
                 END;"
            ))?;
        }
    }
    Ok(())
}

pub fn generation(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'effects_generation'",
        [],
        |row| row.get(0),
    )?)
}

pub fn invalidate(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'effects_generation'",
        [],
    )?;
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
        "INSERT INTO effect_structures(snapshot_id, representation_version, structure_json)
         VALUES (?1, ?2, ?3) ON CONFLICT(snapshot_id) DO UPDATE SET
         representation_version=excluded.representation_version, structure_json=excluded.structure_json",
        (snapshot_id, version, structure_json),
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
    Ok(conn.query_row(
        "SELECT structure_json FROM effect_structures WHERE snapshot_id=?1 AND representation_version=?2",
        (snapshot_id, version), |row| row.get(0),
    ).optional()?)
}

pub fn has_structure(conn: &Connection, snapshot_id: i64, version: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM effect_structures WHERE snapshot_id=?1 AND representation_version=?2)",
        (snapshot_id, version), |row| row.get(0),
    )?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRun {
    pub analysis_id: String,
    pub generation: i64,
    pub semantic_key: String,
    pub scope_key: String,
    pub budget_key: String,
    pub reached_fixed_point: bool,
    pub manifest_json: String,
    pub artifact_json: String,
}

fn read_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRun> {
    Ok(StoredRun {
        analysis_id: row.get(0)?,
        generation: row.get(1)?,
        semantic_key: row.get(2)?,
        scope_key: row.get(3)?,
        budget_key: row.get(4)?,
        reached_fixed_point: row.get(5)?,
        manifest_json: row.get(6)?,
        artifact_json: row.get(7)?,
    })
}

/// Return false on a snapshot race; callers may retry against new inputs.
pub fn publish_run(
    conn: &Connection,
    run: &StoredRun,
    subjects: &[(String, String)],
) -> Result<bool> {
    publish_run_with_issues(conn, run, subjects, &[])
}

pub fn publish_run_with_issues(
    conn: &Connection,
    run: &StoredRun,
    subjects: &[(String, String)],
    issues: &[(String, String)],
) -> Result<bool> {
    publish_run_with_witnesses(conn, run, subjects, issues, &[])
}

pub fn publish_run_with_witnesses(
    conn: &Connection,
    run: &StoredRun,
    subjects: &[(String, String)],
    issues: &[(String, String)],
    witnesses: &[(String, String, String)],
) -> Result<bool> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    if generation(&tx)? != run.generation {
        return Ok(false);
    }
    // V1 does not retain historical source snapshots. Their runs are already
    // unavailable, so reclaim them when publishing against the new generation.
    for table in ["effect_subjects", "effect_issues", "effect_witnesses"] {
        tx.execute(&format!("DELETE FROM {table} WHERE analysis_id IN (SELECT analysis_id FROM effect_runs WHERE generation != ?1)"), [run.generation])?;
    }
    tx.execute(
        "DELETE FROM effect_runs WHERE generation != ?1",
        [run.generation],
    )?;
    tx.execute(
        "DELETE FROM effect_subjects WHERE analysis_id=?1",
        [&run.analysis_id],
    )?;
    tx.execute(
        "DELETE FROM effect_issues WHERE analysis_id=?1",
        [&run.analysis_id],
    )?;
    tx.execute(
        "DELETE FROM effect_witnesses WHERE analysis_id=?1",
        [&run.analysis_id],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO effect_runs VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        rusqlite::params![
            run.analysis_id,
            run.generation,
            run.semantic_key,
            run.scope_key,
            run.budget_key,
            run.reached_fixed_point,
            run.manifest_json,
            run.artifact_json
        ],
    )?;
    for (subject, summary) in subjects {
        tx.execute(
            "INSERT INTO effect_subjects VALUES (?1,?2,?3)",
            rusqlite::params![&run.analysis_id, subject, encode_payload(summary)],
        )?;
    }
    let mut insert = tx.prepare("INSERT INTO effect_issues VALUES (?1,?2,?3)")?;
    for (id, payload) in issues {
        insert.execute(rusqlite::params![
            &run.analysis_id,
            id,
            encode_payload(payload)
        ])?;
    }
    drop(insert);
    let mut insert = tx.prepare("INSERT INTO effect_witnesses VALUES (?1,?2,?3,?4)")?;
    for (subject, effect, witness) in witnesses {
        insert.execute(rusqlite::params![
            &run.analysis_id,
            subject,
            effect,
            encode_payload(witness)
        ])?;
    }
    drop(insert);
    tx.commit()?;
    Ok(true)
}

pub fn load_run(conn: &Connection, analysis_id: &str) -> Result<Option<StoredRun>> {
    Ok(conn.query_row(
        "SELECT analysis_id,generation,semantic_key,scope_key,budget_key,reached_fixed_point,manifest_json,artifact_json
         FROM effect_runs WHERE analysis_id=?1", [analysis_id], read_run,
    ).optional()?)
}

/// Run headers; explanations load the large artifact separately with `load_run`.
pub fn matching_runs(
    conn: &Connection,
    semantic_key: &str,
    generation: i64,
) -> Result<Vec<StoredRun>> {
    let mut stmt = conn.prepare(
        "SELECT analysis_id,generation,semantic_key,scope_key,budget_key,reached_fixed_point,manifest_json,''
         FROM effect_runs WHERE semantic_key=?1 AND generation=?2
         ORDER BY reached_fixed_point DESC, (json_extract(manifest_json,'$.scope.kind')='all') DESC, analysis_id",
    )?;
    let runs = stmt
        .query_map((semantic_key, generation), read_run)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(runs)
}

pub fn load_issues(conn: &Connection, analysis_id: &str, ids: &[String]) -> Result<Vec<String>> {
    let mut select =
        conn.prepare("SELECT issue_json FROM effect_issues WHERE analysis_id=?1 AND issue_id=?2")?;
    ids.iter()
        .map(|id| {
            select
                .query_row((analysis_id, id), |row| {
                    decode_payload(row.get_ref(0)?)
                        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
                })
                .map_err(Into::into)
        })
        .collect()
}

pub fn load_subject(conn: &Connection, analysis_id: &str, subject: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT summary_json FROM effect_subjects WHERE analysis_id=?1 AND subject_key=?2",
        (analysis_id, subject),
        |row| {
            decode_payload(row.get_ref(0)?)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn load_local_matches(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT payload_json FROM effect_local_matches WHERE input_key=?1",
            [key],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn store_local_matches(conn: &Connection, key: &str, payload: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO effect_local_matches VALUES (?1,?2)",
        (key, payload),
    )?;
    Ok(())
}

fn encode_payload(text: &str) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(text.as_bytes())
        .and_then(|_| encoder.finish())
        .expect("writing to an in-memory buffer cannot fail")
}

fn decode_payload(value: ValueRef<'_>) -> Result<String> {
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

pub fn load_witness(
    conn: &Connection,
    analysis_id: &str,
    subject_key: &str,
    effect_id: &str,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT witness_json FROM effect_witnesses WHERE analysis_id=?1 AND subject_key=?2 AND effect_id=?3",
        (analysis_id, subject_key, effect_id),
        |row| decode_payload(row.get_ref(0)?).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into())),
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(generation: i64) -> StoredRun {
        StoredRun {
            analysis_id: "an_test".into(),
            generation,
            semantic_key: "semantic".into(),
            scope_key: "all".into(),
            budget_key: "default".into(),
            reached_fixed_point: true,
            manifest_json: "{}".into(),
            artifact_json: "{}".into(),
        }
    }

    #[test]
    fn snapshot_race_cannot_publish_stale_run() {
        let conn = crate::db::open_test_db().unwrap();
        let pending = run(generation(&conn).unwrap());
        crate::db::write::insert_or_get_spec(&conn, "TEST", "https://test.example", "test")
            .unwrap();
        assert!(!publish_run(&conn, &pending, &[]).unwrap());
        assert!(load_run(&conn, "an_test").unwrap().is_none());
    }

    #[test]
    fn failed_run_publication_is_atomic() {
        let conn = crate::db::open_test_db().unwrap();
        let pending = run(generation(&conn).unwrap());
        let duplicate = vec![("root".into(), "{}".into()), ("root".into(), "{}".into())];
        assert!(publish_run(&conn, &pending, &duplicate).is_err());
        assert!(load_run(&conn, "an_test").unwrap().is_none());
        assert!(publish_run(&conn, &pending, &duplicate[..1]).unwrap());
        assert_eq!(
            load_subject(&conn, "an_test", "root").unwrap().as_deref(),
            Some("{}")
        );
        assert!(load_subject(&conn, "an_test", "unprocessed")
            .unwrap()
            .is_none());
    }

    #[test]
    fn prepared_paths_publish_atomically_and_are_purged_with_the_run() {
        let conn = crate::db::open_test_db().unwrap();
        let pending = run(generation(&conn).unwrap());
        let duplicate = vec![("root".into(), "effect".into(), "{}".into()); 2];
        assert!(publish_run_with_witnesses(&conn, &pending, &[], &[], &duplicate).is_err());
        assert!(load_run(&conn, &pending.analysis_id).unwrap().is_none());
        assert!(publish_run_with_witnesses(&conn, &pending, &[], &[], &duplicate[..1]).unwrap());
        crate::db::schema::purge_if_version_changed(&conn, "test-new-parser").unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM effect_witnesses", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
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

    #[test]
    fn published_rows_are_blobs_and_load_back_as_text() {
        let conn = crate::db::open_test_db().unwrap();
        let run = StoredRun {
            analysis_id: "a1".into(),
            generation: generation(&conn).unwrap(),
            semantic_key: "s".into(),
            scope_key: "sc".into(),
            budget_key: "b".into(),
            reached_fixed_point: true,
            manifest_json: "{}".into(),
            artifact_json: "{}".into(),
        };
        let ok = publish_run_with_witnesses(
            &conn,
            &run,
            &[("subj".into(), "{\"summary\":1}".into())],
            &[("iss".into(), "{\"issue\":1}".into())],
            &[("subj".into(), "eff".into(), "{\"witness\":1}".into())],
        )
        .unwrap();
        assert!(ok);
        let stored_type: String = conn
            .query_row(
                "SELECT typeof(summary_json) FROM effect_subjects WHERE analysis_id='a1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_type, "blob");
        assert_eq!(
            load_subject(&conn, "a1", "subj").unwrap().unwrap(),
            "{\"summary\":1}"
        );
        assert_eq!(
            load_issues(&conn, "a1", &["iss".into()]).unwrap(),
            vec!["{\"issue\":1}".to_string()]
        );
        assert_eq!(
            load_witness(&conn, "a1", "subj", "eff").unwrap().unwrap(),
            "{\"witness\":1}"
        );
    }

    #[test]
    fn legacy_text_rows_still_load() {
        let conn = crate::db::open_test_db().unwrap();
        conn.execute(
            "INSERT INTO effect_runs VALUES ('old', 0, 's', 'sc', 'b', 1, '{}', '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO effect_subjects(analysis_id, subject_key, summary_json) VALUES ('old', 'k', '{\"legacy\":1}')",
            [],
        )
        .unwrap();
        assert_eq!(
            load_subject(&conn, "old", "k").unwrap().unwrap(),
            "{\"legacy\":1}"
        );
    }

    #[test]
    fn rolled_back_corpus_write_does_not_advance_generation() {
        let conn = crate::db::open_test_db().unwrap();
        let before = generation(&conn).unwrap();
        {
            let tx = conn.unchecked_transaction().unwrap();
            crate::db::write::insert_or_get_spec(&tx, "TEST", "https://test.example", "test")
                .unwrap();
            assert!(generation(&tx).unwrap() > before);
        }
        assert_eq!(generation(&conn).unwrap(), before);
    }
}
