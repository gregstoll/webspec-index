#[cfg(test)]
mod tests {
    use crate::effects::catalog::{load_catalog, load_package_files, Catalog};
    use crate::effects::engine::{build_graph, Graph, GraphInput, IndexedAnchor, SourceSpec};
    use crate::parse::steps::extract_step_structure;

    fn engine_fixture_catalog() -> Catalog {
        let package = load_package_files(&[(
            "catalog.yaml",
            include_str!("../../tests/fixtures/effects/engine/catalog.yaml"),
        )])
        .unwrap();
        load_catalog([package]).unwrap()
    }

    fn corpus(names: &[(&str, &str, &str)]) -> Vec<SourceSpec> {
        names
            .iter()
            .map(|(spec, base, html)| {
                let structure = extract_step_structure(html, spec, base, &format!("hash:{spec}"));
                let document = scraper::Html::parse_document(html);
                let parsed = crate::parse::parse_spec_document(&document, spec, base).unwrap();
                let mut anchors: Vec<String> = structure.anchors.clone();
                anchors.extend(parsed.sections.iter().map(|s| s.anchor.clone()));
                anchors.sort();
                anchors.dedup();
                SourceSpec {
                    spec: spec.to_string(),
                    snapshot_sha: format!("hash:{spec}"),
                    base_url: base.to_string(),
                    anchors: anchors
                        .into_iter()
                        .map(|a| IndexedAnchor {
                            url: format!("{base}#{a}"),
                            anchor: a,
                            text: String::new(),
                            idl_kind: None,
                        })
                        .collect(),
                    structure: Some(structure),
                }
            })
            .collect()
    }

    /// DOM, INFRA and URL; INFRA's `b-typed` algorithm is an IDL interface.
    fn multi_corpus() -> Vec<SourceSpec> {
        let mut sources = corpus(&[
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
        ]);
        for anchor in &mut sources[1].anchors {
            if anchor.anchor == "b-typed" {
                anchor.idl_kind = Some("interface".into());
            }
        }
        sources
    }

    /// The graph without `source_order` and with issue numbering replaced by values.
    fn normalized(graph: &Graph) -> serde_json::Value {
        let mut value = serde_json::to_value(graph).unwrap();
        let object = value.as_object_mut().unwrap();
        for key in ["nodes", "edges", "occurrences"] {
            for item in object[key].as_object_mut().unwrap().values_mut() {
                item.as_object_mut().unwrap().remove("source_order");
            }
        }
        let issues: serde_json::Map<String, serde_json::Value> = graph
            .issues
            .iter()
            .map(|(node, ids)| {
                let mut values: Vec<_> = ids
                    .iter()
                    .map(|id| {
                        let issue = &graph.issue_catalog[*id as usize];
                        (
                            serde_json::to_string(&issue.code).unwrap(),
                            issue.message.clone(),
                            issue.site_key.clone(),
                        )
                    })
                    .collect();
                values.sort();
                (node.clone(), serde_json::to_value(values).unwrap())
            })
            .collect();
        serde_json::json!({
            "nodes": object["nodes"],
            "anchor_nodes": object["anchor_nodes"],
            "edges": object["edges"],
            "occurrences": object["occurrences"],
            "issues": issues,
            "definitions": object["definitions"],
            "sites": object["sites"],
            "effect_categories": object["effect_categories"],
        })
    }

    fn check_golden(name: &str, value: &serde_json::Value) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/effects/link")
            .join(format!("{name}.json"));
        let text = serde_json::to_string_pretty(value).unwrap();
        if std::env::var_os("WEBSPEC_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            return;
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "{name}: linked graph differs from the characterized build"
        );
    }

    #[test]
    fn linked_graph_matches_characterized_build() {
        let catalog = engine_fixture_catalog();
        let multi = multi_corpus();
        for environment in ["generic", "browser"] {
            let graph = build_graph(
                GraphInput {
                    sources: &multi,
                    catalog: &catalog,
                    environment,
                },
                None,
            )
            .unwrap();
            check_golden(&format!("multi-{environment}"), &normalized(&graph));
        }
        let single = corpus(&[(
            "TEST",
            "https://example.test/spec",
            include_str!("../../tests/fixtures/effects/engine/acceptance.html"),
        )]);
        let graph = build_graph(
            GraphInput {
                sources: &single,
                catalog: &catalog,
                environment: "generic",
            },
            None,
        )
        .unwrap();
        check_golden("acceptance-generic", &normalized(&graph));
    }
}
