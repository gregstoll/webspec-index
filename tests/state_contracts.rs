#[test]
fn state_schema_is_a_strict_2020_12_document_sharing_effects_definitions() {
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("schemas/state/catalog.schema.json").unwrap(),
    )
    .unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert!(schema.get("$id").is_some());
    let text = serde_json::to_string(&schema).unwrap();
    for def in ["id", "anchor", "match", "capture"] {
        assert!(
            text.contains(&format!("../effects/catalog.schema.json#/$defs/{def}")),
            "{def}"
        );
    }
}
