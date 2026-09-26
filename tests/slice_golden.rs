//! Golden slices (spec §13) over verbatim excerpts of the snapshots the spec was verified on.
use serde::Deserialize;
use webspec_index::content_filter::{transform_content, LinksMode};
use webspec_index::state::slice::select::ViewRequest;
use webspec_index::state::slice::view::{apply_view, SliceResult};
use webspec_index::state::testing::index_offline_with;
use webspec_index::state::StateCatalog;

/// Grammar-only extraction: the goldens must not move when the bundled catalog grows.
fn conn() -> rusqlite::Connection {
    let conn = webspec_index::db::open_in_memory().unwrap();
    let grammar = StateCatalog::default();
    index_offline_with(
        &conn,
        "HTML",
        "https://html.spec.whatwg.org/",
        include_str!("fixtures/slice/golden/html.html"),
        &grammar,
    )
    .unwrap();
    index_offline_with(
        &conn,
        "DOM",
        "https://dom.spec.whatwg.org/",
        include_str!("fixtures/slice/golden/dom.html"),
        &grammar,
    )
    .unwrap();
    conn
}

fn view(
    conn: &rusqlite::Connection,
    spec: &str,
    anchor: &str,
    view: &ViewRequest,
) -> (SliceResult, String) {
    let mut q = webspec_index::query_section_from_conn(conn, spec, anchor)
        .unwrap()
        .unwrap();
    let result = apply_view(conn, &mut q, view).unwrap_or_else(|e| panic!("{spec}#{anchor}: {e}"));
    let registry = webspec_index::spec_registry::SpecRegistry::new();
    let content = transform_content(
        q.content.as_deref().unwrap(),
        LinksMode::Short,
        true,
        &registry,
    );
    (result, content)
}

fn involving(name: &str) -> ViewRequest {
    ViewRequest {
        involving: vec![name.into()],
        ..Default::default()
    }
}

#[test]
fn g1_insert_involving_parent_byte_for_byte() {
    let conn = conn();
    let (result, content) = view(&conn, "DOM", "concept-node-insert", &involving("parent"));
    let part = webspec_index::state::slice::summary::content_part(&result, &content);
    assert_eq!(
        part.trim_end(),
        include_str!("fixtures/slice/golden/g1.md").trim_end()
    );
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(
        json["status"]["counts"],
        serde_json::json!({"steps": 31, "kept": 12, "matched": 10, "inherited": 0, "context": 2, "omitted": 19})
    );
    assert_eq!(
        json["variables"],
        serde_json::json!([{"name": "parent", "basis": "seed"}, {"name": "previousSibling", "basis": "let", "step": "6", "from": ["parent"]}])
    );
}

#[test]
fn g2_navigate_involving_history_handling_byte_for_byte() {
    let conn = conn();
    let (result, content) = view(&conn, "HTML", "navigate", &involving("historyHandling"));
    let part = webspec_index::state::slice::summary::content_part(&result, &content);
    assert_eq!(
        part.trim_end(),
        include_str!("fixtures/slice/golden/g2.md").trim_end()
    );
}

/// `(parent, first, last, steps)`; `steps: None` leaves the count unchecked.
type ExpectedRun = (Option<String>, String, String, Option<u32>);

#[derive(Deserialize)]
struct Case {
    name: String,
    spec: String,
    anchor: String,
    view: ViewRequest,
    #[serde(default)]
    variables: Option<Vec<String>>,
    #[serde(default)]
    variable_set: Option<Vec<String>>,
    #[serde(default)]
    kept: Option<Vec<(String, String)>>,
    #[serde(default)]
    kept_count: Option<usize>,
    #[serde(default)]
    omitted: Option<Vec<ExpectedRun>>,
    #[serde(default)]
    all_reasons: Option<String>,
    #[serde(default)]
    inputs: Option<Vec<String>>,
    #[serde(default)]
    later_definitions: Option<Vec<(String, String, String)>>,
    #[serde(default)]
    stores: Option<Vec<(String, String, Vec<String>)>>,
    #[serde(default)]
    unfollowed_steps: Option<Vec<(String, String, Vec<String>)>>,
}

#[test]
fn g3_to_g6_structured_expectations() {
    #[derive(Deserialize)]
    struct File {
        cases: Vec<Case>,
    }
    let file: File =
        serde_json::from_str(include_str!("fixtures/slice/golden/expectations.json")).unwrap();
    let conn = conn();
    let mut failures = Vec::new();
    for c in &file.cases {
        let (result, _) = view(&conn, &c.spec, &c.anchor, &c.view);
        let json = serde_json::to_value(&result).unwrap();
        let s = &result.slice;
        let mut check = |what: &str, ok: bool, got: String| {
            if !ok {
                failures.push(format!("{} {what}: got {got}", c.name))
            }
        };
        if let Some(want) = &c.variables {
            let got: Vec<String> = s.variables.iter().map(|v| v.name.clone()).collect();
            check("variables", &got == want, format!("{got:?}"));
        }
        if let Some(want) = &c.variable_set {
            let mut got: Vec<String> = s.variables.iter().map(|v| v.name.clone()).collect();
            let mut want = want.clone();
            got.sort();
            want.sort();
            check("variable set", got == want, format!("{got:?}"));
        }
        if let Some(want) = &c.kept {
            let got: Vec<(String, String)> = json["steps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|k| {
                    (
                        k["path"].as_str().unwrap().to_string(),
                        k["role"].as_str().unwrap().to_string(),
                    )
                })
                .collect();
            check("kept", &got == want, format!("{got:?}"));
        }
        if let Some(n) = c.kept_count {
            check(
                "kept count",
                s.steps.len() == n,
                format!("{}", s.steps.len()),
            );
        }
        if let Some(want) = &c.omitted {
            let got: Vec<_> = s
                .omitted
                .iter()
                .map(|r| (r.parent.clone(), r.first.clone(), r.last.clone(), r.steps))
                .collect();
            let ok = got.len() == want.len()
                && got.iter().zip(want).all(|(g, w)| {
                    g.0 == w.0 && g.1 == w.1 && g.2 == w.2 && (w.3.is_none() || w.3 == Some(g.3))
                });
            check("omitted", ok, format!("{got:?}"));
        }
        if let Some(reason) = &c.all_reasons {
            check(
                "reasons",
                s.omitted.iter().all(|r| &r.reason == reason),
                format!("{:?}", s.omitted),
            );
        }
        if let Some(want) = &c.inputs {
            check(
                "inputs",
                s.inputs.as_ref() == Some(want),
                format!("{:?}", s.inputs),
            );
        }
        if let Some(want) = &c.later_definitions {
            let got: Vec<_> = s
                .later_definitions
                .clone()
                .unwrap_or_default()
                .iter()
                .map(|d| {
                    (
                        d.step.clone(),
                        d.variable.clone(),
                        serde_json::to_value(d.kind)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_string(),
                    )
                })
                .collect();
            check("later definitions", &got == want, format!("{got:?}"));
        }
        if let Some(want) = &c.stores {
            let got: Vec<_> = s
                .stores
                .iter()
                .map(|n| (n.step.clone(), n.target.clone(), n.from.clone()))
                .collect();
            check("stores", &got == want, format!("{got:?}"));
        }
        if let Some(want) = &c.unfollowed_steps {
            let got: Vec<_> = s
                .unfollowed
                .iter()
                .map(|u| {
                    (
                        u.step.clone(),
                        serde_json::to_value(u.reason)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_string(),
                        u.variables.clone(),
                    )
                })
                .collect();
            check("unfollowed", &got == want, format!("{got:?}"));
        }
        let omitted: u32 = s.omitted.iter().map(|r| r.steps).sum();
        check(
            "conservation",
            s.steps.len() as u32 + omitted
                == json["status"]["counts"]["steps"].as_u64().unwrap() as u32,
            format!("{omitted}"),
        );
    }
    assert!(
        failures.is_empty(),
        "golden drift — re-verify each item against the spec text:\n{}",
        failures.join("\n")
    );
}
