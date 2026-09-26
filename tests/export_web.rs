use std::path::Path;

use webspec_index::db;
use webspec_index::effects::service::{publish, PublishMode};
use webspec_index::effects::{default_catalog, EffectsOptions};
use webspec_index::export::{export_web, ExportOptions, Manifest};

/// One algorithm per spec, so the publication has rows for each.
fn store_structure(conn: &rusqlite::Connection, snapshot_id: i64, spec: &str, anchor: &str) {
    let html = format!(
        "<div class=algorithm><p>To <dfn id={anchor}>run</dfn>:</p><ol><li>Return.</li></ol></div>"
    );
    let structure = webspec_index::parse::steps::extract_step_structure(
        &html,
        spec,
        "https://example.test/",
        "hash:test",
    );
    db::effects::store_structure(
        conn,
        snapshot_id,
        webspec_index::parse::steps::STRUCTURE_VERSION,
        &serde_json::to_string(&structure).unwrap(),
    )
    .unwrap();
}

fn publish_effects(conn: &rusqlite::Connection) {
    publish(
        conn,
        &default_catalog(&[]).unwrap(),
        &EffectsOptions::default(),
        PublishMode::Incremental,
        Default::default(),
    )
    .unwrap();
}

fn seeded_db(path: &Path) -> (rusqlite::Connection, i64) {
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
    store_structure(&conn, html_snap, "HTML", "navigate");
    store_structure(&conn, rfc_snap, "RFC9110", "section-1");
    db::write::store_memo(&conn, html_snap, b"memo").unwrap();
    conn.execute(
        "INSERT INTO update_checks(spec_id, last_checked) VALUES (1, 'now')",
        [],
    )
    .unwrap();
    (conn, html_snap)
}

fn joined_db(
    dir: &std::path::Path,
    out: &std::path::Path,
    manifest: &Manifest,
) -> rusqlite::Connection {
    let mut joined = Vec::new();
    for i in 0..manifest.chunks {
        joined.extend(std::fs::read(out.join(format!("{i:04}.bin"))).unwrap());
    }
    let joined_path = dir.join("joined.db");
    std::fs::write(&joined_path, &joined).unwrap();
    rusqlite::Connection::open_with_flags(&joined_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
}

fn options() -> ExportOptions {
    ExportOptions {
        providers: vec!["whatwg".into(), "w3c".into(), "tc39".into()],
        specs: vec![],
        chunk_size: 64 * 1024,
        max_total_bytes: 50 * 1024 * 1024,
        page_size: 16384,
    }
}

#[test]
fn export_strips_pr_snapshots_excluded_providers_and_heavy_tables() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("index.db");
    {
        let (conn, html_snap) = seeded_db(&src);
        let state = webspec_index::state::testing::extract_html(
            webspec_index::state::testing::MINI,
            "HTML",
        );
        db::state::store_state(&conn, html_snap, &state).unwrap();
        let structure = webspec_index::parse::steps::extract_step_structure(
            webspec_index::state::testing::MINI,
            "HTML",
            "https://html.spec.whatwg.org/",
            "hash:aaaa",
        );
        db::state::store_slice_indexes(
            &conn,
            html_snap,
            &webspec_index::state::slice::build_slice_indexes(&structure, &state),
        )
        .unwrap();
        publish_effects(&conn);
    }
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

    let conn = joined_db(dir.path(), &out, &manifest);
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
    for gone in [
        "effect_structures",
        "markdown_memo",
        "update_checks",
        "state_models",
    ] {
        assert!(
            !tables.iter().any(|t| t == gone),
            "{gone} should be dropped: {tables:?}"
        );
    }
    // State data (except state_models) must survive the export.
    for kept in [
        "state_fields",
        "state_sites",
        "state_occurrence_counts",
        "state_coverage",
        "state_slices",
    ] {
        let rows: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {kept}"), [], |r| r.get(0))
            .unwrap();
        assert!(rows > 0, "{kept} should have rows in export");
    }
    let snapshots: i64 = conn
        .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(snapshots, 1);
    let specs: i64 = conn
        .query_row("SELECT COUNT(*) FROM specs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(specs, 1);
    let strings = |sql: &str| -> Vec<String> {
        conn.prepare(sql)
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(
        strings("SELECT spec FROM effect_fragments ORDER BY spec"),
        ["HTML", "RFC9110"],
        "every fragment is kept, so read-time linking sees pruned callees"
    );
    assert_eq!(
        strings("SELECT DISTINCT spec FROM effect_summaries ORDER BY spec"),
        ["HTML"]
    );
    let frozen: bool = conn
        .query_row("SELECT frozen FROM effect_publication", [], |r| r.get(0))
        .unwrap();
    assert!(frozen);
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
    let summary: webspec_index::effects::EffectSummaryResult =
        webspec_index::effects::service::get_effect_summary_on(
            &conn,
            &serde_json::from_value(serde_json::json!({
                "schema_version": 1,
                "subject": {"spec": "HTML", "anchor": "navigate"},
                "options": {"mode": "cached"}
            }))
            .unwrap(),
        )
        .unwrap();
    assert!(
        matches!(
            summary.effects_status,
            webspec_index::effects::EffectsStatus::Ready { .. }
        ),
        "the frozen publication stays current after pruning: {summary:?}"
    );
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
    drop(seeded_db(&src));
    let err = export_web(&src, &dir.path().join("web"), &options()).unwrap_err();
    assert!(err.to_string().contains("effects --all"), "{err}");
}

#[test]
fn export_fails_the_size_gate() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("index.db");
    let (conn, _) = seeded_db(&src);
    publish_effects(&conn);
    drop(conn);
    let mut opts = options();
    opts.max_total_bytes = 1024;
    let err = export_web(&src, &dir.path().join("web"), &opts).unwrap_err();
    assert!(err.to_string().contains("exceeds"), "{err}");
}

#[test]
fn export_specs_filter_keeps_only_listed_specs() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("index.db");
    let (conn, _) = seeded_db(&src);
    // Seed a second spec (DOM, provider=whatwg), then publish.
    let dom = db::write::insert_or_get_spec(&conn, "DOM", "https://dom.spec.whatwg.org/", "whatwg")
        .unwrap();
    let dom_snap = db::write::insert_snapshot(&conn, dom, "hash:dddd", "2026-09-01").unwrap();
    conn.execute(
        "INSERT INTO sections(snapshot_id, anchor, title, content_text, section_type) VALUES (?1, 'concept-tree', 'Tree', 'body', 'heading')",
        [dom_snap],
    )
    .unwrap();
    publish_effects(&conn);
    drop(conn);

    let out = dir.path().join("web");
    let mut opts = options();
    opts.specs = vec!["html".into()]; // filter to HTML only (case-insensitive)
    let manifest = export_web(&src, &out, &opts).unwrap();

    assert_eq!(manifest.specs.len(), 1, "only HTML should be in the export");
    assert_eq!(manifest.specs[0].name, "HTML");
}

#[test]
fn exported_db_answers_state_requests_without_state_models() {
    use webspec_index::state::testing::{base_url, index_offline, QUERY_DOM, QUERY_HTML};
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("index.db");
    {
        let (conn, _) = seeded_db(&src);
        for (spec, html) in [("DOM", QUERY_DOM), ("HTML", QUERY_HTML)] {
            index_offline(&conn, spec, base_url(spec), html).unwrap();
        }
        conn.execute(
            "UPDATE specs SET provider = 'whatwg' WHERE name IN ('DOM', 'HTML')",
            [],
        )
        .unwrap();
        publish_effects(&conn);
    }
    let out = dir.path().join("web");
    let manifest = export_web(&src, &out, &options()).unwrap();
    let conn = joined_db(dir.path(), &out, &manifest);
    let has_models: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'state_models'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_models, 0);
    let v: serde_json::Value = serde_json::from_str(&webspec_index::api::handle_json(
        &conn,
        r#"{"type":"state","selector":"Element.node document"}"#,
    ))
    .unwrap();
    assert_eq!(v["type"], "state_field", "{v}");
    assert_eq!(v["result"]["writes"].as_array().unwrap().len(), 2);
}

#[test]
fn exported_db_answers_views() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("index.db");
    {
        let (conn, _) = seeded_db(&src);
        let html = r##"<div class="algorithm"><p>To <dfn id="go">go</dfn> given a <var>foo</var>:</p><ol><li><p>Let <var>a</var> be <var>foo</var>.</p></li><li><p>Return.</p></li></ol></div>"##;
        webspec_index::state::testing::index_offline(&conn, "T", "https://t.example/", html)
            .unwrap();
        conn.execute("UPDATE specs SET provider = 'whatwg' WHERE name = 'T'", [])
            .unwrap();
        publish_effects(&conn);
    }
    let out = dir.path().join("web");
    let manifest = export_web(&src, &out, &options()).unwrap();
    let conn = joined_db(dir.path(), &out, &manifest);
    let v: serde_json::Value = serde_json::from_str(&webspec_index::api::handle_json(
        &conn,
        r#"{"type":"query","target":"T#go","view":{"involving":["foo"]}}"#,
    ))
    .unwrap();
    assert_eq!(v["type"], "query", "{v}");
    assert_eq!(v["result"]["slice"]["status"]["counts"]["omitted"], 1);
}
