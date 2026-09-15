use std::path::Path;

use webspec_index::db;
use webspec_index::export::{export_web, ExportOptions};

fn seeded_db(path: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(path).unwrap();
    db::schema::initialize_schema(&conn).unwrap();
    db::schema::run_migrations(&conn).unwrap();
    let html =
        db::write::insert_or_get_spec(&conn, "HTML", "https://html.spec.whatwg.org/", "whatwg")
            .unwrap();
    let rfc = db::write::insert_or_get_spec(
        &conn,
        "RFC9110",
        "https://www.rfc-editor.org/rfc/rfc9110.html",
        "ietf",
    )
    .unwrap();
    let html_snap = db::write::insert_snapshot(&conn, html, "hash:aaaa", "2026-09-01").unwrap();
    let html_pr = db::write::insert_snapshot(&conn, html, "pr:1234", "2026-09-02").unwrap();
    conn.execute(
        "UPDATE snapshots SET pr_number = 1234 WHERE id = ?1",
        [html_pr],
    )
    .unwrap();
    let rfc_snap = db::write::insert_snapshot(&conn, rfc, "hash:cccc", "2026-09-01").unwrap();
    for (snap, anchor) in [
        (html_snap, "navigate"),
        (html_pr, "navigate"),
        (rfc_snap, "section-1"),
    ] {
        conn.execute(
            "INSERT INTO sections(snapshot_id, anchor, title, content_text, section_type) VALUES (?1, ?2, 'T', 'body text', 'heading')",
            (snap, anchor),
        )
        .unwrap();
    }
    conn.execute_batch(
        "INSERT INTO effect_structures(snapshot_id, representation_version, structure_json) VALUES (1, 'v', '{}');
         INSERT INTO effect_local_matches(input_key, payload_json) VALUES ('k', '{}');
         INSERT INTO update_checks(spec_id, last_checked) VALUES (1, 'now');
         INSERT INTO effect_runs(analysis_id, generation, semantic_key, scope_key, budget_key, reached_fixed_point, manifest_json, artifact_json)
           VALUES ('a1', (SELECT CAST(value AS INTEGER) FROM meta WHERE key='effects_generation'), 's', 'sc', 'b', 1, '{}', 'BIGARTIFACT');
         INSERT INTO effect_subjects(analysis_id, subject_key, summary_json) VALUES ('a1', 'k', '{}');",
    )
    .unwrap();
    conn
}

fn options() -> ExportOptions {
    ExportOptions {
        providers: vec!["whatwg".into(), "w3c".into(), "tc39".into()],
        chunk_size: 64 * 1024,
        max_total_bytes: 50 * 1024 * 1024,
        page_size: 16384,
    }
}

#[test]
fn export_strips_pr_snapshots_excluded_providers_and_heavy_tables() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("index.db");
    drop(seeded_db(&src));
    let out = dir.path().join("web");
    let manifest = export_web(&src, &out, &options()).unwrap();

    assert_eq!(manifest.specs.len(), 1);
    assert_eq!(manifest.specs[0].name, "HTML");
    assert_eq!(manifest.specs[0].sha, "hash:aaaa");
    assert!(manifest.chunks >= 1);
    assert_eq!(
        manifest.size,
        std::fs::read_dir(&out)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "bin"))
            .map(|e| e.metadata().unwrap().len())
            .sum::<u64>()
    );
    let written: webspec_index::export::Manifest =
        serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(written.sha256, manifest.sha256);

    let mut joined = Vec::new();
    for i in 0..manifest.chunks {
        joined.extend(std::fs::read(out.join(format!("{i:04}.bin"))).unwrap());
    }
    let joined_path = dir.path().join("joined.db");
    std::fs::write(&joined_path, &joined).unwrap();
    let conn = rusqlite::Connection::open_with_flags(
        &joined_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let page_size: u32 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap();
    assert_eq!(page_size, 16384);
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for gone in ["effect_structures", "effect_local_matches", "update_checks"] {
        assert!(
            !tables.iter().any(|t| t == gone),
            "{gone} should be dropped: {tables:?}"
        );
    }
    let snapshots: i64 = conn
        .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(snapshots, 1);
    let specs: i64 = conn
        .query_row("SELECT COUNT(*) FROM specs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(specs, 1);
    let sections: i64 = conn
        .query_row("SELECT COUNT(*) FROM sections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sections, 1);
    let fts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sections_fts WHERE sections_fts MATCH 'body'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts, 1);
    let artifact: String = conn
        .query_row("SELECT artifact_json FROM effect_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(artifact, "");
    let has_index: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_refs_to'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_index, 1);
}

#[test]
fn export_refuses_without_prepared_effects() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("index.db");
    let conn = seeded_db(&src);
    conn.execute_batch("DELETE FROM effect_subjects; DELETE FROM effect_runs;")
        .unwrap();
    drop(conn);
    let err = export_web(&src, &dir.path().join("web"), &options()).unwrap_err();
    assert!(
        err.to_string().contains("effects --all --summary-only"),
        "{err}"
    );
}

#[test]
fn export_fails_the_size_gate() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("index.db");
    drop(seeded_db(&src));
    let mut opts = options();
    opts.max_total_bytes = 1024;
    let err = export_web(&src, &dir.path().join("web"), &opts).unwrap_err();
    assert!(err.to_string().contains("exceeds"), "{err}");
}
