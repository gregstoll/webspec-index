use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use webspec_index::state::query::{query, SiteInfo, StateQueryOptions, StateResponse};

#[derive(Deserialize)]
struct Expectations {
    fields: Vec<FieldExpectation>,
}

#[derive(Deserialize)]
struct FieldExpectation {
    field: String,
    writes: Vec<String>,
    inits: Vec<String>,
    #[serde(default)]
    unclassified: Vec<String>,
    #[serde(default)]
    negatives: BTreeMap<String, String>,
}

/// `SPEC#subject:step` for numbered sites, `SPEC#subject (role)` for prose steps
/// ("method", "setter", …), `SPEC#subject` otherwise. Shared with `state::testing`.
fn site_key(site: &SiteInfo) -> String {
    webspec_index::state::testing::site_key(
        &site.spec,
        &site.subject,
        site.step_path.as_deref(),
        site.role.as_deref(),
    )
}

fn compare(
    field: &str,
    kind: &str,
    sites: &[SiteInfo],
    want: &[String],
    failures: &mut Vec<String>,
) {
    let got: BTreeSet<String> = sites.iter().map(site_key).collect();
    let want: BTreeSet<String> = want.iter().cloned().collect();
    for extra in got.difference(&want) {
        let text = sites
            .iter()
            .find(|s| &site_key(s) == extra)
            .map(|s| s.text.as_str())
            .unwrap_or("");
        failures.push(format!("{field}: unexpected {kind} {extra}: {text}"));
    }
    for missing in want.difference(&got) {
        failures.push(format!("{field}: missing {kind} {missing}"));
    }
}

pub fn check(conn: &rusqlite::Connection) -> Vec<String> {
    let expectations: Expectations =
        serde_json::from_str(include_str!("fixtures/state/golden/expectations.json")).unwrap();
    let options = StateQueryOptions {
        include_inits: true,
        unclassified: true,
        limit: Some(1000),
    };
    let mut failures = Vec::new();
    for e in &expectations.fields {
        let response = query(conn, &e.field, &options)
            .unwrap_or_else(|err| panic!("{}: {}", e.field, err.message));
        let StateResponse::Field(f) = response else {
            panic!("{} is not a field", e.field)
        };
        compare(&e.field, "write", &f.writes, &e.writes, &mut failures);
        let inits = f.inits.unwrap_or_default();
        compare(&e.field, "init", &inits, &e.inits, &mut failures);
        compare(
            &e.field,
            "unclassified",
            &f.unclassified.items,
            &e.unclassified,
            &mut failures,
        );
        for (site, class) in &e.negatives {
            let classes = webspec_index::state::testing::occurrence_classes(conn, &e.field, site);
            let ok = if class == "absent" {
                classes.is_empty()
            } else {
                classes.contains(class)
            };
            if !ok {
                failures.push(format!(
                    "{}: {site} expected {class}, found {classes:?}",
                    e.field
                ));
            }
        }
    }
    failures
}

#[test]
fn golden_writers_on_excerpt_fixtures() {
    let conn = webspec_index::db::open_in_memory().unwrap();
    webspec_index::state::testing::index_offline(
        &conn,
        "HTML",
        "https://html.spec.whatwg.org/",
        include_str!("fixtures/state/golden/html.html"),
    )
    .unwrap();
    webspec_index::state::testing::index_offline(
        &conn,
        "DOM",
        "https://dom.spec.whatwg.org/",
        include_str!("fixtures/state/golden/dom.html"),
    )
    .unwrap();
    let failures = check(&conn);
    assert!(
        failures.is_empty(),
        "golden drift — re-verify each site against the spec text:\n{}",
        failures.join("\n")
    );
}
