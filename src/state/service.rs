//! Native entry points: ensure the selector's spec is indexed, then query.
use crate::state::query::{query, StateError, StateQueryOptions, StateResponse};

pub async fn state(
    selector: &str,
    options: &StateQueryOptions,
) -> anyhow::Result<Result<StateResponse, StateError>> {
    let conn = crate::db::open_or_create_db()?;
    if let Some(spec) = selector_spec(selector) {
        let registry = crate::spec_registry::SpecRegistry::new();
        crate::ensure_indexed_for_spec_name(&conn, &registry, &spec, None).await?;
    }
    Ok(query(&conn, selector, options))
}

/// The spec a `SPEC#…` selector or spec URL names; `TYPE` selectors span every indexed spec.
fn selector_spec(selector: &str) -> Option<String> {
    let trimmed = selector.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return crate::parse_spec_anchor(trimmed)
            .ok()
            .map(|(spec, _, _)| spec);
    }
    trimmed.split_once('#').map(|(spec, _)| spec.to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::selector_spec;

    #[test]
    fn selector_spec_covers_anchor_url_and_type_forms() {
        assert_eq!(selector_spec(" html#x ").as_deref(), Some("HTML"));
        assert_eq!(
            selector_spec("https://dom.spec.whatwg.org/#concept-node-document").as_deref(),
            Some("DOM")
        );
        assert_eq!(selector_spec("Element.node document"), None);
    }
}
