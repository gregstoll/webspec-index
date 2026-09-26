//! Native entry point: validate, fetch, apply a view, transform content, return JSON.
use crate::{
    content_filter::{transform_content, LinksMode},
    state::slice::{
        apply_view,
        select::{SliceError, ViewRequest},
        validate_shape,
    },
};
use anyhow::Result;
use serde_json::Value;

pub async fn query_view(
    spec_anchor: &str,
    view: &ViewRequest,
    links: LinksMode,
    no_notes: bool,
) -> Result<Result<Value, SliceError>> {
    let normalized = match validate_shape(view) {
        Ok(n) => n,
        Err(e) => return Ok(Err(e)),
    };

    let mut query = crate::query_section(spec_anchor, None).await?;

    let conn = rusqlite::Connection::open(crate::db::get_db_path())?;

    let slice = match apply_view(&conn, &mut query, &normalized) {
        Ok(s) => s,
        Err(e) => return Ok(Err(e)),
    };

    if links != LinksMode::Full || no_notes {
        if let Some(content) = query.content.as_deref() {
            let registry = crate::spec_registry::SpecRegistry::new();
            query.content = Some(transform_content(content, links, no_notes, &registry));
        }
    }

    let mut value = serde_json::to_value(&query)?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("slice".to_string(), serde_json::to_value(&slice)?);
    }
    Ok(Ok(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::slice::select::SliceErrorCode;

    #[tokio::test]
    async fn empty_view_is_invalid_before_any_network() {
        let result = query_view(
            "HTML#navigate",
            &ViewRequest::default(),
            LinksMode::Short,
            false,
        )
        .await
        .unwrap();
        let err = result.unwrap_err();
        assert_eq!(err.code, SliceErrorCode::InvalidSelector);
    }
}
