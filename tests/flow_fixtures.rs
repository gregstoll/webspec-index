//! Fixture corpus tests for `flow::extract_flow`.
//!
//! Each `tests/fixtures/flow/*.md` file is a real algorithm's stored markdown,
//! dumped with `webspec-index query SPEC#anchor --effects off --format json`
//! (field `content`). The matching `*.expected.json` records the expected
//! structure from `extract_flow` with an empty call list.
//!
//! To regenerate all expected files after a behaviour change run:
//!   `cargo run --example gen_flow_fixtures`
//!
//! Expected JSON shape:
//! ```json
//! {
//!   "nodes": <total count>,
//!   "edges": <total count>,
//!   "issues": [<issue code strings>],
//!   "head_nodes": [{"id": "...", "kind": "..."}, ...],
//!   "head_edge_kinds": ["next", "then", ...]
//! }
//! ```
//! `head_nodes` is the first 6 nodes (id + kind); `head_edge_kinds` is the
//! first 6 edge kinds.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use webspec_index::{flow, model};

#[derive(Debug, Serialize, Deserialize)]
struct ExpectedHead {
    id: String,
    kind: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Expected {
    nodes: usize,
    edges: usize,
    issues: Vec<String>,
    head_nodes: Vec<ExpectedHead>,
    head_edge_kinds: Vec<String>,
}

fn run_fixture(
    stem: &str,
) -> (
    Vec<model::FlowNode>,
    Vec<model::FlowEdge>,
    Vec<model::FlowIssue>,
) {
    let md_path = Path::new("tests/fixtures/flow").join(format!("{stem}.md"));
    let markdown = fs::read_to_string(&md_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", md_path.display()));
    flow::extract_flow(&markdown, &[])
}

fn check_fixture(stem: &str) {
    let expected_path = Path::new("tests/fixtures/flow").join(format!("{stem}.expected.json"));
    let expected_raw = fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", expected_path.display()));
    let expected: Expected = serde_json::from_str(&expected_raw)
        .unwrap_or_else(|e| panic!("could not parse {}: {e}", expected_path.display()));

    let (nodes, edges, issues) = run_fixture(stem);

    assert_eq!(nodes.len(), expected.nodes, "{stem}: node count mismatch");
    assert_eq!(edges.len(), expected.edges, "{stem}: edge count mismatch");

    let issue_codes: Vec<String> = issues.iter().map(|i| i.code.clone()).collect();
    assert_eq!(issue_codes, expected.issues, "{stem}: issue codes mismatch");

    let head_nodes: Vec<ExpectedHead> = nodes
        .iter()
        .take(6)
        .map(|n| ExpectedHead {
            id: n.id.clone(),
            kind: format!("{:?}", n.kind).to_lowercase(),
        })
        .collect();
    for (i, (got, exp)) in head_nodes
        .iter()
        .zip(expected.head_nodes.iter())
        .enumerate()
    {
        assert_eq!(got.id, exp.id, "{stem}: head_nodes[{i}].id mismatch");
        // kind comparison: normalise case
        let got_kind = format!("{:?}", nodes[i].kind);
        assert_eq!(
            got_kind.to_lowercase(),
            exp.kind.to_lowercase(),
            "{stem}: head_nodes[{i}].kind mismatch"
        );
    }

    let head_edge_kinds: Vec<String> = edges
        .iter()
        .take(6)
        .map(|e| format!("{:?}", e.kind).to_lowercase())
        .collect();
    assert_eq!(
        head_edge_kinds, expected.head_edge_kinds,
        "{stem}: head_edge_kinds mismatch"
    );
}

#[test]
fn fixture_navigate() {
    check_fixture("navigate");
}

#[test]
fn fixture_concept_node_insert() {
    check_fixture("concept-node-insert");
}

#[test]
fn fixture_concept_fetch() {
    check_fixture("concept-fetch");
}

#[test]
fn fixture_tostring() {
    check_fixture("tostring");
}
