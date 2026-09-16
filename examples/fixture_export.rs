use std::path::PathBuf;

use webspec_index::db;
use webspec_index::export::{export_web, ExportOptions};
use webspec_index::model::{ParsedReference, ParsedSection, RefKind, SectionType};

fn main() {
    let out_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/fixture-export"));

    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("fixture.db");

    let conn = rusqlite::Connection::open(&db_path).expect("open db");
    db::schema::initialize_schema(&conn).expect("schema");
    db::schema::run_migrations(&conn).expect("migrations");

    let html =
        db::write::insert_or_get_spec(&conn, "HTML", "https://html.spec.whatwg.org/", "whatwg")
            .expect("insert HTML spec");
    let dom = db::write::insert_or_get_spec(&conn, "DOM", "https://dom.spec.whatwg.org/", "whatwg")
        .expect("insert DOM spec");

    let html_snap =
        db::write::insert_snapshot(&conn, html, "hash:fixture", "2026-09-15").expect("html snap");
    let dom_snap =
        db::write::insert_snapshot(&conn, dom, "hash:domfix", "2026-09-15").expect("dom snap");

    db::write::insert_sections_bulk(
        &conn,
        html_snap,
        &[
            ParsedSection {
                anchor: "browsing".into(),
                title: Some("Browsing contexts".into()),
                content_text: None,
                section_type: SectionType::Heading,
                parent_anchor: None,
                prev_anchor: None,
                next_anchor: None,
                depth: Some(2),
                number: None,
            },
            ParsedSection {
                anchor: "navigate".into(),
                title: Some("Navigate".into()),
                content_text: Some(
                    "To **navigate** a [navigable](https://html.spec.whatwg.org#navigable) \
                     to a [tree](https://dom.spec.whatwg.org/#concept-tree):\n\n\
                     1. Let x be y.\n2. Return."
                        .into(),
                ),
                section_type: SectionType::Algorithm,
                parent_anchor: Some("browsing".into()),
                prev_anchor: None,
                next_anchor: None,
                depth: Some(3),
                number: None,
            },
        ],
    )
    .expect("insert HTML sections");

    db::write::insert_sections_bulk(
        &conn,
        dom_snap,
        &[ParsedSection {
            anchor: "concept-tree".into(),
            title: Some("Trees".into()),
            content_text: None,
            section_type: SectionType::Heading,
            parent_anchor: None,
            prev_anchor: None,
            next_anchor: None,
            depth: Some(2),
            number: None,
        }],
    )
    .expect("insert DOM sections");

    db::write::insert_refs_bulk(
        &conn,
        html_snap,
        &[ParsedReference {
            from_anchor: "navigate".into(),
            to_spec: "DOM".into(),
            to_anchor: "concept-tree".into(),
            step_path: Some("1".into()),
            step_text: Some("Let x be y.".into()),
            guard_path: vec![],
            call_site_id: None,
            kind: RefKind::Step,
        }],
    )
    .expect("insert refs");

    conn.execute_batch(
        "INSERT INTO effect_runs(analysis_id, generation, semantic_key, scope_key, budget_key, \
                                  reached_fixed_point, manifest_json, artifact_json) \
         VALUES ('fixture', (SELECT CAST(value AS INTEGER) FROM meta WHERE key='effects_generation'), \
                 'nav', 'sc', 'b', 1, '{}', '{}'); \
         INSERT INTO effect_subjects(analysis_id, subject_key, summary_json) \
         VALUES ('fixture', 'navigate', '{}');",
    )
    .expect("effect rows");

    drop(conn);

    let manifest = export_web(
        &db_path,
        &out_dir,
        &ExportOptions {
            chunk_size: 64 * 1024,
            ..Default::default()
        },
    )
    .expect("export_web");

    println!("{}", serde_json::to_string_pretty(&manifest).unwrap());
}
