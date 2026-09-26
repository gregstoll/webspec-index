//! Native entry points: load `--rules` packages, ensure the selector's spec is
//! indexed, then query.
use crate::state::catalog::{self, StateCatalog};
use crate::state::query::{
    query_with_rules, StateError, StateErrorCode, StateQueryOptions, StateResponse,
};

pub async fn state(
    selector: &str,
    options: &StateQueryOptions,
    rule_paths: &[String],
) -> anyhow::Result<Result<StateResponse, StateError>> {
    let extra = match rule_packages(rule_paths) {
        Ok(extra) => extra,
        Err(error) => return Ok(Err(error)),
    };
    let conn = crate::db::open_or_create_db()?;
    if let Some(spec) = selector_spec(selector) {
        let registry = crate::spec_registry::SpecRegistry::new();
        crate::ensure_indexed_for_spec_name(&conn, &registry, &spec, None).await?;
    }
    Ok(query_with_rules(&conn, selector, options, &extra))
}

/// The rules of every `--rules` package directory; a package that declares
/// types or fields is rejected.
fn rule_packages(paths: &[String]) -> Result<StateCatalog, StateError> {
    let invalid = |path: &str, message: String| {
        StateError::new(
            StateErrorCode::InvalidRules,
            format!("--rules {path}: {message}"),
        )
    };
    let mut catalogs = Vec::with_capacity(paths.len());
    for path in paths {
        let package =
            catalog::load_state_package(path).map_err(|error| invalid(path, error.to_string()))?;
        catalog::rules_only(&package).map_err(|error| invalid(path, error.message))?;
        catalogs.push(package);
    }
    catalog::merge(catalogs).map_err(|error| invalid(&paths.join(", "), error.to_string()))
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
    use super::{rule_packages, selector_spec};
    use crate::state::query::StateErrorCode;

    #[test]
    fn selector_spec_covers_anchor_url_and_type_forms() {
        assert_eq!(selector_spec(" html#x ").as_deref(), Some("HTML"));
        assert_eq!(
            selector_spec("https://dom.spec.whatwg.org/#concept-node-document").as_deref(),
            Some("DOM")
        );
        assert_eq!(selector_spec("Element.node document"), None);
    }

    #[test]
    fn rule_packages_reject_declarations() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("state")).unwrap();
        std::fs::write(
            dir.path().join("state/f.yaml"),
            "schema: 1\npackage: p\nfields:\n  - id: f\n    field: HTML#f\n    owner: [HTML#o]\n    expect_text: 'x'\n    reason: r\n",
        )
        .unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let error = rule_packages(&[path]).unwrap_err();
        assert_eq!(error.code, StateErrorCode::InvalidRules);
        assert!(error.message.contains("only rules"));
    }
}
