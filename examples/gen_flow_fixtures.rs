//! Helper to regenerate `tests/fixtures/flow/*.expected.json`.
//! Run with: `cargo run --example gen_flow_fixtures`

use std::fs;
use std::path::Path;

use serde::Serialize;
use webspec_index::{flow, model};

#[derive(Serialize)]
struct HeadNode {
    id: String,
    kind: String,
}

#[derive(Serialize)]
struct Expected {
    nodes: usize,
    edges: usize,
    issues: Vec<String>,
    head_nodes: Vec<HeadNode>,
    head_edge_kinds: Vec<String>,
}

fn generate(stem: &str) {
    let md_path = Path::new("tests/fixtures/flow").join(format!("{stem}.md"));
    let markdown = fs::read_to_string(&md_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", md_path.display()));

    let (nodes, edges, issues) = flow::extract_flow(&markdown, &[]);

    let expected = Expected {
        nodes: nodes.len(),
        edges: edges.len(),
        issues: issues.iter().map(|i| i.code.clone()).collect(),
        head_nodes: nodes
            .iter()
            .take(6)
            .map(|n| HeadNode {
                id: n.id.clone(),
                kind: kind_str(n.kind),
            })
            .collect(),
        head_edge_kinds: edges
            .iter()
            .take(6)
            .map(|e| edge_kind_str(e.kind))
            .collect(),
    };

    let json = serde_json::to_string_pretty(&expected).unwrap();
    let out_path = Path::new("tests/fixtures/flow").join(format!("{stem}.expected.json"));
    fs::write(&out_path, json).unwrap();
    println!("wrote {}", out_path.display());
}

fn kind_str(k: model::FlowNodeKind) -> String {
    match k {
        model::FlowNodeKind::Step => "step",
        model::FlowNodeKind::Branch => "branch",
        model::FlowNodeKind::Loop => "loop",
        model::FlowNodeKind::Parallel => "parallel",
        model::FlowNodeKind::Terminal => "terminal",
        model::FlowNodeKind::External => "external",
    }
    .to_string()
}

fn edge_kind_str(k: model::FlowEdgeKind) -> String {
    match k {
        model::FlowEdgeKind::Next => "next",
        model::FlowEdgeKind::Then => "then",
        model::FlowEdgeKind::Else => "else",
        model::FlowEdgeKind::Loop => "loop",
        model::FlowEdgeKind::Jump => "jump",
        model::FlowEdgeKind::Unknown => "unknown",
        model::FlowEdgeKind::Call => "call",
    }
    .to_string()
}

fn main() {
    for stem in &[
        "navigate",
        "concept-node-insert",
        "concept-fetch",
        "tostring",
    ] {
        generate(stem);
    }
    println!("done");
}
