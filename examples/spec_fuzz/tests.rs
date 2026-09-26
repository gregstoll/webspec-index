//! Harness self-tests (design §11): oracle regions, quiet on clean input, fires on planted defects,
//! dedupe and determinism.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::OptionalExtension;
use webspec_index::content_filter::LinksMode;
use webspec_index::effects::bundled::default_catalog;
use webspec_index::effects::model::EffectsOptions;
use webspec_index::effects::service::{publish, PublishMode};
use webspec_index::model::ParsedSection;

use webspec_index::state::slice::select::{slice as slice_fn, ViewRequest};
use webspec_index::state::slice::SliceIndex;
use webspec_index::state::testing::slice_index;

use crate::check::{self, SpecInput, SpecResult};
use crate::invariants::{
    self, check_e1, check_e2, check_e3, query, slice as inv_slice, Ctx, Invariant, Outcome,
    SectionCtx, SpecCtx,
};
use crate::oracle::{self, Shape};

const SPEC: &str = "HTML";
const BASE: &str = "https://html.spec.whatwg.org";

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(rel),
    )
    .unwrap()
}

fn ctx(html: &str) -> SpecCtx {
    ctx_for(html, SPEC, BASE)
}

fn ctx_for(html: &str, spec: &str, base: &str) -> SpecCtx {
    let mut r = SpecResult::default();
    check::build_ctx(
        SpecInput {
            spec,
            base_url: base,
            sha: "hash:test",
            html,
            db: None,
            stored: None,
            with_structure: true,
        },
        &mut r,
    )
    .unwrap()
}

fn inv(id: &str) -> &'static dyn Invariant {
    invariants::by_id(id).unwrap()
}

fn check_on(c: &SpecCtx, section: &ParsedSection, id: &str) -> Outcome {
    let s = SectionCtx::new(c, section).expect("locatable");
    inv(id).check(Ctx::Section(&s))
}

fn check_anchor(c: &SpecCtx, anchor: &str, id: &str) -> Outcome {
    check_on(c, c.section(anchor).expect("section"), id)
}

/// Run `id` on `anchor` with its stored content replaced by `plant(content)`.
fn planted(c: &SpecCtx, anchor: &str, id: &str, plant: impl FnOnce(&str) -> String) -> Outcome {
    let mut s = c.section(anchor).unwrap().clone();
    let content = s.content_text.clone().unwrap_or_default();
    let planted = plant(&content);
    assert_ne!(planted, content, "the plant must change the content");
    s.content_text = Some(planted);
    check_on(c, &s, id)
}

fn fails(o: &Outcome) -> bool {
    matches!(o, Outcome::Fail(_))
}

fn shape(c: &SpecCtx, anchor: &str) -> Shape {
    SectionCtx::new(c, c.section(anchor).unwrap())
        .unwrap()
        .region
        .shape
}

const CLEAN: [&str; 10] = [
    "algorithms/bikeshed_algorithm.html",
    "algorithms/wattsi_navigate.html",
    "algorithms/wattsi_dl_switch.html",
    "algorithms/wattsi_ul_algorithm.html",
    "definitions/wattsi_property_list.html",
    "headings/bikeshed_heading.html",
    "headings/wattsi_heading.html",
    "ecmarkup/tostring.html",
    "idl/interface.html",
    "idl/dictionary.html",
];

/// Product defects a clean fixture may carry, as (fixture, invariant). The regression suite tracks
/// the defect itself; here it only must not count as noise.
const KNOWN: [(&str, &str); 1] = [
    // defect 4: <code>-wrapped links are not IR links
    ("ecmarkup/tostring.html", "S3"),
];

// ── 1. Oracle regions ───────────────────────────────────────────────────────────────────────────

#[test]
fn regions_follow_the_rule_table() {
    let c = ctx(&fixture("algorithms/bikeshed_algorithm.html"));
    assert_eq!(shape(&c, "concept-ordered-set-parser"), Shape::AlgoDiv);

    let c = ctx(&fixture("algorithms/wattsi_navigate.html"));
    assert_eq!(shape(&c, "navigate"), Shape::AlgoSibling);

    let c = ctx(&fixture("definitions/wattsi_property_list.html"));
    assert_eq!(shape(&c, "is-closing"), Shape::DefItem);
    assert_eq!(shape(&c, "nav-id"), Shape::DefItem);

    let c = ctx(&fixture("headings/bikeshed_heading.html"));
    assert_eq!(shape(&c, "trees"), Shape::Heading);

    let c = ctx(&fixture("ecmarkup/tostring.html"));
    assert_eq!(shape(&c, "sec-tostring"), Shape::EmuClause);

    let c = ctx(&fixture("idl/interface.html"));
    assert_eq!(shape(&c, "event"), Shape::IdlPre);

    let c = ctx(&fixture("definitions/wattsi_definition.html"));
    assert_eq!(shape(&c, "in-parallel"), Shape::DefDfnOnly);

    let c = ctx(
        r##"<p><dfn data-dfn-type="interface" id="gpu"><code>GPU</code></dfn> is the entry point.</p>"##,
    );
    assert_eq!(shape(&c, "gpu"), Shape::IdlNoPre);

    let c = ctx(
        r##"<table><tr id="row"><td>cell</td></tr></table><dl><dt id="term">term</dt><dd>d</dd></dl>"##,
    );
    assert_eq!(shape(&c, "row"), Shape::Element);
    assert_eq!(shape(&c, "term"), Shape::Element);

    let c = ctx(
        r##"<section id="s"><div class="header-wrapper"><h2 id="intro">Intro</h2><a class="self-link" href="#s"></a></div><p>Prose.</p></section>"##,
    );
    assert_eq!(shape(&c, "intro"), Shape::HeadingWrapped);

    let c = ctx(
        r##"<html class="RFC"><body><section id="section-1"><h2 id="n">1. Intro</h2><p>Own.</p><section id="section-1.1"><h3>Sub</h3><p>Nested.</p></section></section></body></html>"##,
    );
    let s = SectionCtx::new(&c, c.section("section-1").unwrap()).unwrap();
    assert_eq!(s.region.shape, Shape::RfcSection);
    assert_eq!(oracle::words(&s.src_text().text), vec!["own"]);

    // A dfn in an algorithm div that is not the div's first dfn and has no list of its own.
    let c = ctx(
        r##"<div class="algorithm"><p>To <dfn id="a">run</dfn>:</p><ol><li>Go.</li></ol><p>The <dfn id="b">thing</dfn> is nice.</p></div>"##,
    );
    assert_eq!(shape(&c, "a"), Shape::AlgoDiv);
    assert_eq!(shape(&c, "b"), Shape::AlgoNone);
}

#[test]
fn heading_region_is_the_own_prose() {
    let html = r##"<h2 id="a">A</h2><p>Own prose.</p><h3 id="b">B</h3><p>Sub prose.</p>"##;
    let c = ctx(html);
    let s = SectionCtx::new(&c, c.section("a").unwrap()).unwrap();
    assert_eq!(oracle::words(&s.src_text().text), vec!["own", "prose"]);
}

#[test]
fn algorithm_div_region_includes_trailing_content() {
    let html = r##"<div class="algorithm"><p>To <dfn id="x">x</dfn>:</p><ul><li>input</li></ul><ol><li>Step.</li></ol><p class="note">Trailing <a href="#t">note</a>.</p></div>"##;
    let c = ctx(html);
    let s = SectionCtx::new(&c, c.section("x").unwrap()).unwrap();
    let text = oracle::words(&s.src_text().text);
    assert!(text.contains(&"step".to_string()) && text.contains(&"trailing".to_string()));
    assert_eq!(s.src_text().expected_links(), vec![format!("{BASE}#t")]);
}

// ── 2. Quiet on clean input ─────────────────────────────────────────────────────────────────────

#[test]
fn v1_invariants_are_quiet_on_clean_fixtures() {
    let mut noisy = Vec::new();
    for f in CLEAN {
        let c = ctx(&fixture(f));
        for section in c.unique_sections() {
            let Some(s) = SectionCtx::new(&c, section) else {
                continue;
            };
            for i in invariants::all()
                .into_iter()
                .filter(|i| !i.report_only() && !i.spec_level())
            {
                if KNOWN.contains(&(f, i.id())) {
                    continue;
                }
                if let Outcome::Fail(fail) = i.check(Ctx::Section(&s)) {
                    noisy.push(format!(
                        "{f} {} {}: {:?}",
                        section.anchor,
                        i.id(),
                        fail.items
                    ));
                }
            }
        }
    }
    assert!(noisy.is_empty(), "{noisy:#?}");
}

// ── 3. Fires on planted defects ─────────────────────────────────────────────────────────────────

#[test]
fn c1_fires_on_a_deleted_link_and_on_a_link_inside_a_code_span() {
    let c = ctx(&fixture("algorithms/wattsi_navigate.html"));
    assert!(!fails(&check_anchor(&c, "navigate", "C1")));
    let url = format!("({BASE}#assert)");
    assert!(fails(&planted(&c, "navigate", "C1", |md| md.replacen(
        &format!("[Assert]{url}"),
        "Assert",
        1
    ))));
    assert!(fails(&planted(&c, "navigate", "C1", |md| {
        md.replacen(&format!("[Assert]{url}"), &format!("`[Assert]{url}`"), 1)
    })));
}

#[test]
fn c2_fires_on_a_dropped_cell_and_merged_paragraphs() {
    let html = r##"<h2 id="h">T</h2><p>First paragraph ends here.</p><p>Second paragraph.</p>
        <table><tr><td>cell one</td><td>cell two</td></tr></table>"##;
    let c = ctx(html);
    assert!(!fails(&check_anchor(&c, "h", "C2")));
    assert!(fails(
        &planted(&c, "h", "C2", |md| md.replace("cell two", ""))
    ));
    assert!(fails(&planted(&c, "h", "C2", |md| md
        .replace("here.\n\nSecond", "here.Second"))));
}

#[test]
fn c3_fires_on_empty_content() {
    let c = ctx(&fixture("definitions/wattsi_definition.html"));
    assert!(!fails(&check_anchor(&c, "in-parallel", "C3")));
    assert!(fails(&planted(&c, "in-parallel", "C3", |_| String::new())));
}

#[test]
fn r1_r2_fire_on_a_removed_ref() {
    let mut c = ctx(&fixture("algorithms/wattsi_navigate.html"));
    assert!(!fails(&check_anchor(&c, "navigate", "R1")));
    assert!(!fails(&check_anchor(&c, "navigate", "R2")));
    let refs = c.refs_by_from.get_mut("navigate").unwrap();
    let first = refs.iter().next().unwrap().clone();
    refs.remove(&first);
    assert!(fails(&check_anchor(&c, "navigate", "R1")));
    assert!(fails(&check_anchor(&c, "navigate", "R2")));
}

#[test]
fn s1_s2_fire_on_a_removed_nested_step() {
    let mut c = ctx(&fixture("algorithms/wattsi_navigate.html"));
    assert!(!fails(&check_anchor(&c, "navigate", "S1")));
    assert!(!fails(&check_anchor(&c, "navigate", "S2")));
    assert!(fails(&planted(&c, "navigate", "S1", |md| {
        let i = md.find("    2. ").expect("nested step 2");
        let end = md[i..].find("\n\n").map(|e| i + e).unwrap_or(md.len());
        format!("{}{}", &md[..i], &md[end..])
    })));
    let st = c.structure.as_mut().unwrap();
    let alg = st
        .algorithms
        .iter_mut()
        .find(|a| a.source.section_anchor == "navigate")
        .unwrap();
    let nested = alg.steps.iter().position(|s| s.path.len() == 2).unwrap();
    alg.steps.remove(nested);
    assert!(fails(&check_anchor(&c, "navigate", "S2")));
}

#[test]
fn s3_fires_when_a_segment_loses_its_links() {
    let mut c = ctx(&fixture("algorithms/wattsi_navigate.html"));
    assert!(!fails(&check_anchor(&c, "navigate", "S3")));
    let st = c.structure.as_mut().unwrap();
    let alg = st
        .algorithms
        .iter_mut()
        .find(|a| a.source.section_anchor == "navigate")
        .unwrap();
    let seg = alg
        .segments
        .iter_mut()
        .find(|s| !s.links.is_empty())
        .unwrap();
    seg.links.clear();
    assert!(fails(&check_anchor(&c, "navigate", "S3")));
}

/// Remove every link span whose href ends with `suffix`, wherever the IR keeps it.
fn strip_ir_links(alg: &mut webspec_index::parse::steps::StructuralAlgorithm, suffix: &str) {
    fn rec(v: &mut serde_json::Value, suffix: &str) {
        match v {
            serde_json::Value::Array(a) => {
                a.retain(|x| {
                    !x.get("href")
                        .and_then(|h| h.as_str())
                        .is_some_and(|h| h.ends_with(suffix))
                });
                a.iter_mut().for_each(|x| rec(x, suffix));
            }
            serde_json::Value::Object(m) => m.values_mut().for_each(|x| rec(x, suffix)),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(&*alg).unwrap();
    rec(&mut v, suffix);
    *alg = serde_json::from_value(v).unwrap();
}

#[test]
fn s3_fires_when_a_dt_link_is_missing_from_the_ir() {
    // The `new Document, with:` shape: a field link on a <dl class=props> term.
    let html = r##"<div data-algorithm=""><p>To <dfn id="pick">pick</dfn>:</p><ol>
        <li><p>Let <var>d</var> be a new <a href="#document">Document</a>, with:</p><dl class="props">
        <dt><a href="#she-url">URL</a></dt><dd><var>url</var></dd>
        </dl></li></ol></div>"##;
    let mut c = ctx(html);
    let st = c.structure.as_mut().unwrap();
    let alg = st
        .algorithms
        .iter_mut()
        .find(|a| a.source.section_anchor == "pick")
        .unwrap();
    strip_ir_links(alg, "#she-url");
    match check_anchor(&c, "pick", "S3") {
        Outcome::Fail(f) => assert!(
            f.items.iter().any(|e| e.construct.contains("dt")),
            "{:?}",
            f.items
        ),
        o => panic!("{o:?}"),
    }
}

#[test]
fn m0_reports_panicking_modes() {
    assert!(query::panicking_modes(|_, _| String::new()).is_empty());
    let modes = query::panicking_modes(|m, n| {
        if m == LinksMode::Short && n {
            panic!("planted");
        }
        String::new()
    });
    assert_eq!(modes, vec!["short+no-notes"]);
}

#[test]
fn m1_fires_on_a_fragment_the_url_parser_cannot_round_trip() {
    let c = ctx_for(
        r##"<p><dfn id="plain">plain</dfn> text.</p>"##,
        "HTML",
        "https://html.spec.whatwg.org",
    );
    assert!(!fails(&check_anchor(&c, "plain", "M1")));
    // Non-ASCII fragments come back percent-encoded (a product defect M1 exists to catch).
    let c = ctx_for(
        r##"<p><dfn id="caf①">café</dfn> text.</p>"##,
        "HTML",
        "https://html.spec.whatwg.org",
    );
    assert!(fails(&check_anchor(&c, "caf①", "M1")));
}

#[test]
fn m2_fires_on_a_wrong_short_destination() {
    let reg = webspec_index::spec_registry::SpecRegistry::new();
    let full = "See [navigate](https://html.spec.whatwg.org/#navigate).";
    assert!(query::m2_items(full, "See [navigate](HTML#navigate).", &reg).is_empty());
    assert!(!query::m2_items(full, "See [navigate](HTML#reload).", &reg).is_empty());
}

#[test]
fn m4_fires_on_indentation_residue() {
    let full = "1. First [a](https://x.test/a).\n\n    > **Note:** A [note](https://x.test/n).\n\n2. Second [b](https://x.test/b).\n";
    let clean = "1. First [a](https://x.test/a).\n\n2. Second [b](https://x.test/b).\n";
    assert!(query::m4a_items(full, clean).is_empty());
    // Defect 5: the note's indentation stays and turns item 2 into an indented code block.
    let residue = "1. First [a](https://x.test/a).\n\n        2. Second [b](https://x.test/b).\n";
    assert!(!query::m4a_items(full, residue).is_empty());
}

#[test]
fn q1_fires_on_a_truncated_refs_result() {
    let lib: Vec<(String, String)> = (0..6).map(|i| ("S".to_string(), format!("a{i}"))).collect();
    assert!(query::q1_items(&lib, &lib).is_empty());
    assert!(!query::q1_items(&lib, &lib[..5]).is_empty());
}

#[test]
fn n1_fires_on_a_broken_prev_pointer() {
    let html = r##"<h2 id="a">A</h2><p>x</p><h2 id="b">B</h2><p>y</p>"##;
    let mut c = ctx(html);
    assert!(!fails(&check_anchor(&c, "a", "N1")));
    let i = c.first["b"];
    c.parsed.sections[i].prev_anchor = Some("elsewhere".into());
    assert!(fails(&check_anchor(&c, "a", "N1")));
}

#[test]
fn d1_fires_on_an_altered_stored_row() {
    let c = ctx(&fixture("algorithms/wattsi_navigate.html"));
    let stored = |c: &SpecCtx| crate::index::Stored {
        sections: c.unique_sections().cloned().collect(),
        refs: c
            .parsed
            .references
            .iter()
            .map(crate::index::RefRow::from_parsed)
            .collect(),
        structure_json: c
            .structure
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap()),
    };
    assert!(invariants::d1_diff(&c, &stored(&c)).is_empty());
    let mut altered = stored(&c);
    altered.sections[0].content_text = Some("stale".into());
    assert!(!invariants::d1_diff(&c, &altered).is_empty());
}

#[test]
fn t1_flags_a_dfn_whose_list_sibling_is_not_steps() {
    let c = ctx(&fixture("definitions/wattsi_property_list.html"));
    // `navigable` is followed by a property list, so the product types it Algorithm (defect 13).
    assert!(fails(&check_anchor(&c, "navigable", "T1")));
    let c = ctx(&fixture("algorithms/wattsi_navigate.html"));
    assert!(!fails(&check_anchor(&c, "navigate", "T1")));
}

#[test]
fn a1_lists_unindexed_parameter_dfns() {
    let c = ctx(&fixture("definitions/wattsi_property_list.html"));
    match check_anchor(&c, "close-a-top-level-traversable", "A1") {
        Outcome::Fail(f) => assert!(
            f.items.iter().any(|e| e.construct == "dfn-parameter"),
            "{:?}",
            f.items
        ),
        o => panic!("{o:?}"),
    }
}

#[test]
fn o1_fires_on_reordered_content() {
    let html = r##"<h2 id="h">T</h2><p>Alpha beta gamma.</p><p>Delta epsilon zeta.</p>"##;
    let c = ctx(html);
    assert!(!fails(&check_anchor(&c, "h", "O1")));
    assert!(fails(&planted(&c, "h", "O1", |_| {
        "Delta epsilon zeta.\n\nAlpha beta gamma.".into()
    })));
}

// ── 4. Historical defect ────────────────────────────────────────────────────────────────────────

#[test]
fn c1_catches_the_pre_b14577d_props_table() {
    let html = r##"<div data-algorithm=""><p>To <dfn id="mk">make a document</dfn>:</p><ol>
        <li><p>Let <var>d</var> be a new <a href="#document">Document</a>, with:</p>
        <dl class="props"><dt><a href="#concept-document-type">type</a></dt><dd>"<code>html</code>"</dd>
        <dt><a href="#concept-document-mode">mode</a></dt><dd>"<code>quirks</code>"</dd></dl></li></ol></div>"##;
    let c = ctx(html);
    assert!(!fails(&check_anchor(&c, "mk", "C1")));
    // The renderer before b14577d emitted text-only cells.
    let old = format!(
        "To **make a document**:\n\n1. Let *d* be a new [Document]({BASE}#document), with:\n\n    | Field | Value |\n    |-------|-------|\n    | type | \"html\" |\n    | mode | \"quirks\" |\n"
    );
    assert!(fails(&planted(&c, "mk", "C1", |_| old)));
}

// ── 5. Dedupe and determinism ───────────────────────────────────────────────────────────────────

fn corpus_findings(jobs_order: &[&str]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for f in jobs_order {
        let html = fixture(f);
        let r = check::check_spec(
            SpecInput {
                spec: SPEC,
                base_url: BASE,
                sha: "hash:test",
                html: &html,
                db: None,
                stored: None,
                with_structure: true,
            },
            &invariants::all(),
        );
        out.extend(
            r.findings
                .into_iter()
                .map(|x| (f.to_string(), x.signature, x.anchor)),
        );
    }
    out.sort();
    out
}

#[test]
fn findings_are_deterministic() {
    let forward = corpus_findings(&CLEAN);
    let mut reversed = CLEAN;
    reversed.reverse();
    assert_eq!(forward, corpus_findings(&reversed));
    assert_eq!(forward, corpus_findings(&CLEAN));
}

#[test]
fn one_record_per_signature_with_the_smallest_representative() {
    let raw = |anchor: &str, html: &str| check::RawFinding {
        invariant: "C1",
        report_only: false,
        signature: "C1|heading/heading|missing:table>th>a".into(),
        spec: "TEST".into(),
        anchor: anchor.into(),
        section_type: "heading".into(),
        shape: "heading".into(),
        evidence: Vec::new(),
        expected: serde_json::json!({}),
        actual: serde_json::json!({}),
        region_html: html.into(),
    };
    let manifest = crate::report::Manifest {
        run_id: "t".into(),
        harness_rev: "r".into(),
        dirty: false,
        index_version: "v".into(),
        structure_version: "s".into(),
        db: String::new(),
        invariants: Vec::new(),
        snapshots: Default::default(),
    };
    let mut store = crate::report::Store::default();
    store.add(
        &raw("outer", "<h2>big region with many nodes</h2>"),
        &manifest,
        Path::new("d"),
    );
    store.add(&raw("inner", "<h3>small</h3>"), &manifest, Path::new("d"));
    assert_eq!(store.findings.len(), 1);
    let rec = store.findings.values().next().unwrap();
    assert_eq!((rec.count, rec.anchor.as_str()), (2, "inner"));
    assert_eq!(rec.more_anchors, vec!["TEST#outer"]);
}

// ── 6. E1–E3: effects storage invariants ───────────────────────────────────────────────────────

/// The multi/ fixtures used by Task 7 and later effect tests.
const MULTI: [(&str, &str, &str); 3] = [
    (
        "DOM",
        "https://dom.spec.whatwg.org/",
        include_str!("../../tests/fixtures/effects/multi/alpha.html"),
    ),
    (
        "INFRA",
        "https://infra.spec.whatwg.org/",
        include_str!("../../tests/fixtures/effects/multi/beta.html"),
    ),
    (
        "URL",
        "https://url.spec.whatwg.org/",
        include_str!("../../tests/fixtures/effects/multi/gamma.html"),
    ),
];

/// Seed an in-memory DB with the multi/ fixtures, publish effects, and return the connection
/// together with a map from spec name to snapshot_id.
fn effects_db() -> (rusqlite::Connection, BTreeMap<String, i64>) {
    use webspec_index::db;
    use webspec_index::db::effects as storage;
    use webspec_index::parse;
    use webspec_index::parse::steps::{extract_step_structure, STRUCTURE_VERSION};

    let conn = db::open_in_memory().expect("open in-memory DB");
    let mut ids = BTreeMap::new();
    for (spec, base_url, html) in MULTI {
        let spec_id = db::write::insert_or_get_spec(&conn, spec, base_url, "test").unwrap();
        let snap_id =
            db::write::insert_snapshot(&conn, spec_id, "hash:test", "2026-01-01").unwrap();
        let parsed = parse::parse_spec(html, spec, base_url).unwrap();
        db::write::insert_sections_bulk(&conn, snap_id, &parsed.sections).unwrap();
        db::write::insert_refs_bulk(&conn, snap_id, &parsed.references).unwrap();
        let structure = extract_step_structure(html, spec, base_url, "hash:test");
        storage::store_structure(
            &conn,
            snap_id,
            STRUCTURE_VERSION,
            &serde_json::to_string(&structure).unwrap(),
        )
        .unwrap();
        ids.insert(spec.to_owned(), snap_id);
    }
    let catalog = default_catalog(&[]).unwrap();
    publish(
        &conn,
        &catalog,
        &EffectsOptions::default(),
        PublishMode::Rebuild,
        BTreeMap::new(),
    )
    .expect("publish effects");
    (conn, ids)
}

#[test]
fn e1_passes_on_consistent_db() {
    let (conn, ids) = effects_db();
    for (spec, &snap_id) in &ids {
        let base = MULTI
            .iter()
            .find(|(s, _, _)| *s == spec)
            .map(|(_, b, _)| *b)
            .unwrap_or("");
        assert!(
            matches!(check_e1(&conn, spec, snap_id, base), Outcome::Pass),
            "E1 failed for {spec}"
        );
    }
}

#[test]
fn e1_fails_after_corrupting_fragment() {
    let (conn, ids) = effects_db();
    let snap_dom = ids["DOM"];
    let snap_infra = ids["INFRA"];

    let infra_payload: Vec<u8> = conn
        .query_row(
            "SELECT payload FROM effect_fragments WHERE snapshot_id=?1",
            [snap_infra],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE effect_fragments SET payload=?1 WHERE snapshot_id=?2",
        rusqlite::params![infra_payload, snap_dom],
    )
    .unwrap();

    assert!(
        matches!(
            check_e1(&conn, "DOM", snap_dom, "https://dom.spec.whatwg.org/"),
            Outcome::Fail(_)
        ),
        "E1 should fail after corrupting DOM fragment with INFRA payload"
    );
    assert!(
        matches!(
            check_e1(&conn, "INFRA", snap_infra, "https://infra.spec.whatwg.org/"),
            Outcome::Pass
        ),
        "E1 should still pass for INFRA"
    );
}

#[test]
fn e2_passes_on_consistent_db() {
    let (conn, _) = effects_db();
    assert!(
        matches!(check_e2(&conn, "DOM"), Outcome::Pass),
        "E2 failed for DOM"
    );
}

#[test]
fn e2_fails_after_deleting_summary_row() {
    let (conn, _) = effects_db();
    let key_to_delete: Option<String> = conn
        .query_row(
            "SELECT subject_key FROM effect_summaries WHERE spec='DOM' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    let key_to_delete = key_to_delete.expect("DOM has at least one summary row");
    let deleted = conn
        .execute(
            "DELETE FROM effect_summaries WHERE subject_key=?1",
            rusqlite::params![key_to_delete],
        )
        .unwrap();
    assert!(deleted > 0, "no row deleted");
    assert!(
        matches!(check_e2(&conn, "DOM"), Outcome::Fail(_)),
        "E2 should fail after deleting a DOM summary row"
    );
}

#[test]
fn e3_passes_on_consistent_db() {
    let (conn, _) = effects_db();
    assert!(
        matches!(
            check_e3(&conn, "DOM"),
            Outcome::Pass | Outcome::NotApplicable
        ),
        "E3 failed for DOM"
    );
}

#[test]
fn e3_fails_after_replacing_summary_payload() {
    let (conn, _) = effects_db();
    let dom_key: Option<String> = conn
        .query_row(
            "SELECT subject_key FROM effect_summaries WHERE spec='DOM' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    let Some(dom_key) = dom_key else {
        return;
    };
    let other_payload: Option<Vec<u8>> = conn
        .query_row(
            "SELECT payload FROM effect_summaries WHERE spec='INFRA' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    let Some(other_payload) = other_payload else {
        return;
    };
    conn.execute(
        "UPDATE effect_summaries SET payload=?1, digest=x'deadbeef' WHERE subject_key=?2",
        rusqlite::params![other_payload, dom_key],
    )
    .unwrap();
    assert!(
        matches!(check_e3(&conn, "DOM"), Outcome::Fail(_)),
        "E3 should detect a corrupted summary payload (wrong digest)"
    );
}

// ── 7. Slice invariants L1–L3 ───────────────────────────────────────────────────────────────────

const SLICE_OK: &str = r##"<div class="algorithm"><p>To <dfn id="go">go</dfn> given a <var>foo</var>:</p><ol>
<li><p>Let <var>a</var> be <var>foo</var>.</p></li>
<li><p><a href="#rm">Remove</a> it with <a href="#rm-so"><var>foo</var></a> set to true.</p></li>
<li><p>If <var>a</var> is null:</p><ol><li><p>Return.</p></li></ol></li>
</ol></div><p><dfn id="rm">remove</dfn> <dfn id="rm-so">so</dfn></p>"##;

fn src(path: &str, vars: &[&str], text: &str) -> inv_slice::SourceStep {
    inv_slice::SourceStep {
        path: path.into(),
        own_vars: vars.iter().map(|v| v.to_string()).collect(),
        own_text: text.into(),
    }
}

fn view(index: &SliceIndex, var: &str) -> webspec_index::state::slice::select::Slice {
    slice_fn(
        index,
        &ViewRequest {
            involving: vec![var.into()],
            ..Default::default()
        },
    )
    .unwrap()
}

fn source_steps_of(c: &SpecCtx, anchor: &str) -> Vec<inv_slice::SourceStep> {
    let section = c.section(anchor).expect("section");
    let sctx = SectionCtx::new(c, section).expect("locatable");
    inv_slice::source_steps(&sctx)
}

#[test]
fn slice_invariants_l1_to_l3_are_quiet_on_a_clean_algorithm() {
    let c = ctx(SLICE_OK);
    for id in ["L1", "L2", "L3"] {
        assert!(matches!(check_anchor(&c, "go", id), Outcome::Pass), "{id}");
    }
}

#[test]
fn l1_ignores_named_argument_labels() {
    let c = ctx(SLICE_OK);
    let steps = source_steps_of(&c, "go");
    assert_eq!(
        steps[1].own_vars,
        Vec::<String>::new(),
        "the label <a><var>foo</var></a> in step 2 is not a mention"
    );
}

#[test]
fn uncovered_reports_a_source_var_the_index_does_not_mention() {
    let index = slice_index("go", &[("1", &["a"]), ("2", &["foo"])], &[], &[]);
    let source = [
        src("1", &["a", "foo"], "Let a be foo."),
        src("2", &["foo"], "Return foo."),
    ];
    assert_eq!(inv_slice::uncovered(&index, &source), ["missing:1"]);
    let good = slice_index("go", &[("1", &["a", "foo"]), ("2", &["foo"])], &[], &[]);
    assert!(inv_slice::uncovered(&good, &source).is_empty());
}

#[test]
fn conservation_gap_reports_a_dropped_run() {
    let index = slice_index("go", &[("1", &["x"]), ("2", &[]), ("3", &["x"])], &[], &[]);
    let mut s = view(&index, "x");
    assert_eq!(inv_slice::conservation_gap(&s, 3), None);
    s.omitted.clear();
    assert_eq!(
        inv_slice::conservation_gap(&s, 3).as_deref(),
        Some("count:2+0!=3")
    );
}

#[test]
fn open_prefixes_reports_a_kept_path_without_its_prefix() {
    let index = slice_index(
        "go",
        &[("1", &[]), ("1.1", &["x"]), ("2", &[]), ("2.1", &[])],
        &[],
        &[],
    );
    let mut s = view(&index, "x");
    assert!(inv_slice::open_prefixes(&s).is_empty());
    s.steps.retain(|k| k.path != "1");
    s.omitted[0].parent = Some("9".into());
    let gaps = inv_slice::open_prefixes(&s);
    assert!(
        gaps.contains(&"1.1".to_string()) && gaps.contains(&"run-parent:9".to_string()),
        "{gaps:?}"
    );
}
