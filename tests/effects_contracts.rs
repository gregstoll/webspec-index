use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use webspec_index::effects::{
    load_catalog, load_catalog_sources, load_package, load_package_files, EffectSummaryResult,
    EffectsRequest, EffectsStatus, ExplainEffectsRequest, ExplainEffectsResult,
    RecomputeEffectsRequest, RecomputeEffectsResult,
};

const FIXTURES: &str = "tests/fixtures/effects/contracts";

fn round_trip<T: DeserializeOwned + Serialize>(name: &str) {
    let path = format!("{FIXTURES}/{name}");
    let input: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let typed: T = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), input, "{name}");
}

#[test]
fn schemas_are_valid_json_documents_with_strict_roots() {
    for name in ["catalog", "request", "result", "fixture"] {
        let path = format!("schemas/effects/{name}.schema.json");
        let schema: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert!(schema.get("$id").is_some());
    }
}

#[test]
fn golden_requests_and_results_round_trip() {
    round_trip::<EffectsRequest>("summary-request.json");
    round_trip::<ExplainEffectsRequest>("explain-request.json");
    round_trip::<RecomputeEffectsRequest>("recompute-request.json");
    round_trip::<EffectSummaryResult>("summary-result.json");
    round_trip::<ExplainEffectsResult>("explain-result.json");
    round_trip::<RecomputeEffectsResult>("recompute-result.json");

    let summary: EffectSummaryResult = serde_json::from_str(
        &fs::read_to_string(format!("{FIXTURES}/summary-result.json")).unwrap(),
    )
    .unwrap();
    summary.validate().unwrap();
    let explanation: ExplainEffectsResult = serde_json::from_str(
        &fs::read_to_string(format!("{FIXTURES}/explain-result.json")).unwrap(),
    )
    .unwrap();
    explanation.validate().unwrap();
}

#[test]
fn result_status_rejects_fields_for_another_state() {
    let pending_with_result = serde_json::json!({
        "state": "pending",
        "semantics": "may",
        "issues": [],
        "omitted": 0,
        "coverage": "complete",
        "analysis_id": "an_0123456789abcdef"
    });
    assert!(serde_json::from_value::<EffectsStatus>(pending_with_result).is_err());

    let ready_without_identity = serde_json::json!({
        "state": "ready",
        "semantics": "may",
        "coverage": "complete",
        "issues": [],
        "omitted": 0
    });
    assert!(serde_json::from_value::<EffectsStatus>(ready_without_identity).is_err());
}

#[test]
fn unsupported_wire_versions_are_rejected_during_deserialization() {
    let mut request: Value = serde_json::from_str(
        &fs::read_to_string(format!("{FIXTURES}/summary-request.json")).unwrap(),
    )
    .unwrap();
    request["schema_version"] = 2.into();
    assert!(serde_json::from_value::<EffectsRequest>(request).is_err());
}

#[test]
fn document_examples_form_one_extensible_catalog() {
    let base = load_package(format!("{FIXTURES}/catalog/example")).unwrap();
    let extension = load_package(format!("{FIXTURES}/catalog/extension")).unwrap();
    let catalog = load_catalog([base, extension]).unwrap();
    assert_eq!(catalog.effects.len(), 3);
    assert_eq!(catalog.rules().count(), 4);
    assert_eq!(catalog.summaries().count(), 1);
    assert_eq!(catalog.implementations().count(), 1);
}

#[test]
fn embedded_and_directory_sources_share_catalog_validation() {
    let embedded = &[&[
        (
            "effects.yaml",
            include_str!("fixtures/effects/contracts/catalog/example/effects.yaml"),
        ),
        (
            "summaries.yaml",
            include_str!("fixtures/effects/contracts/catalog/example/summaries.yaml"),
        ),
        (
            "implementations.yaml",
            include_str!("fixtures/effects/contracts/catalog/example/implementations.yaml"),
        ),
    ][..]][..];
    let additional = [PathBuf::from(format!("{FIXTURES}/catalog/extension"))];
    let catalog = load_catalog_sources(embedded, &additional).unwrap();
    assert_eq!(
        catalog
            .packages
            .iter()
            .map(|package| package.id.as_str())
            .collect::<Vec<_>>(),
        vec!["example", "example-extension"]
    );

    let direct = load_package_files(embedded[0]).unwrap();
    assert_eq!(direct.content_digest, catalog.packages[0].content_digest);
}

#[test]
fn invalid_catalog_fixtures_have_actionable_errors() {
    let invalid = format!("{FIXTURES}/catalog/invalid");
    for (name, needle) in [
        ("unknown-field.yaml", "unknown field"),
        ("duplicate-key.yaml", "duplicated key"),
        ("alias.yaml", "anchors are not supported"),
    ] {
        let content = fs::read_to_string(format!("{invalid}/{name}")).unwrap();
        let error = load_package_files(&[(name, &content)])
            .expect_err("invalid catalog unexpectedly loaded")
            .to_string();
        assert!(error.contains(name), "{error}");
        assert!(error.contains(needle), "{error}");
    }
}

#[test]
fn fixture_golden_uses_the_versioned_fixture_shape() {
    let fixture: Value = serde_json::from_str(
        &fs::read_to_string(format!("{FIXTURES}/direct-two-sites.case.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(fixture["id"], "direct-two-sites");
    assert_eq!(fixture["expect_local_effects"].as_array().unwrap().len(), 2);
    assert_eq!(
        fixture["forbidden_effects"][0]["params"]["name"],
        Value::Null
    );
}

#[test]
fn catalog_endpoint_aliases_use_registry_identity() {
    let package = load_package_files(&[(
        "aliases.yaml",
        r#"
schema: 1
package: aliases
effects:
  test.effect:
    category: tests
    label: test effect
rules:
  - id: html-alias
    match: {anchor: 'html#navigate'}
    emit: {kind: test.effect}
  - id: ecma-alias
    match: {anchor: 'ECMA262#sec-hostenqueuepromisejob'}
    emit: {kind: test.effect}
"#,
    )])
    .unwrap();
    let catalog = load_catalog([package]).unwrap();
    let anchors: Vec<_> = catalog
        .rules()
        .map(|rule| rule.match_spec.anchor.as_ref().unwrap().as_identity())
        .collect();
    assert!(anchors.contains(&"HTML#navigate".to_string()));
    assert!(anchors.contains(&"ECMA-262#sec-hostenqueuepromisejob".to_string()));
}
