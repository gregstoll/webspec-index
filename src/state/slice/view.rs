//! Slice results and applying a view to a query result (spec §8, §9).
use rusqlite::Connection;
use serde::Serialize;

use crate::{
    db::{
        queries::get_snapshot,
        state::{has_slice_rows, load_slice_index},
    },
    model::QueryResult,
};

use super::{
    render_view, slice, validate_shape, RenderedView, Rendering, Slice, SliceError, SliceErrorCode,
    SliceIndex, StepRole, ViewRequest,
};

#[derive(Debug, Clone, Serialize)]
pub struct SliceCounts {
    pub steps: usize,
    pub kept: usize,
    pub matched: usize,
    pub inherited: usize,
    pub context: usize,
    pub omitted: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceIssue {
    RenderUnaligned,
}

#[derive(Debug, Clone, Serialize)]
pub struct SliceStatus {
    pub semantics: &'static str,
    pub scope: &'static str,
    pub rendering: Rendering,
    pub counts: SliceCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_items: Option<usize>,
    pub issues: Vec<SliceIssue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SliceResult {
    pub algorithm: String,
    pub status: SliceStatus,
    #[serde(flatten)]
    pub slice: Slice,
}

/// Builds a `SliceResult` from the algorithm id, its index, the computed slice and the rendered view.
pub fn slice_result(
    algorithm: String,
    index: &SliceIndex,
    slice: Slice,
    rendered: &RenderedView,
) -> SliceResult {
    let kept = slice.steps.len();
    let matched = slice
        .steps
        .iter()
        .filter(|s| s.role == StepRole::Match)
        .count();
    let inherited = slice
        .steps
        .iter()
        .filter(|s| s.role == StepRole::Inherited)
        .count();
    let context = slice
        .steps
        .iter()
        .filter(|s| s.role == StepRole::Context)
        .count();
    let omitted: u32 = slice.omitted.iter().map(|r| r.steps).sum();

    let (markdown_items, issues) = if rendered.rendering == Rendering::Unaligned {
        (
            Some(rendered.markdown_items),
            vec![SliceIssue::RenderUnaligned],
        )
    } else {
        (None, vec![])
    };

    SliceResult {
        algorithm,
        status: SliceStatus {
            semantics: "may",
            scope: "algorithm",
            rendering: rendered.rendering,
            counts: SliceCounts {
                steps: index.steps.len(),
                kept,
                matched,
                inherited,
                context,
                omitted,
            },
            markdown_items,
            issues,
        },
        slice,
    }
}

fn unavailable(spec: &str, message: impl Into<String>) -> SliceError {
    let _ = spec;
    SliceError {
        code: SliceErrorCode::Unavailable,
        message: message.into(),
        candidates: vec![],
    }
}

/// Applies `view` to `query`: validates it, checks the slice index is available, computes the
/// slice, and replaces `query.content` with the rendered view. On error, `query` is unchanged.
pub fn apply_view(
    conn: &Connection,
    query: &mut QueryResult,
    view: &ViewRequest,
) -> Result<SliceResult, SliceError> {
    // Step 1: validate shape — no DB reads needed.
    let view = validate_shape(view)?;

    // Step 2: get trunk snapshot and verify the sha matches.
    let snapshot_id = get_snapshot(conn, &query.spec)
        .map_err(|e| unavailable(&query.spec, e.to_string()))?
        .ok_or_else(|| {
            unavailable(
                &query.spec,
                format!(
                    "views need the indexed trunk snapshot of {}; \
                     PR previews and stale results have no slice index",
                    query.spec
                ),
            )
        })?;

    let snapshot_sha: String = conn
        .query_row(
            "SELECT sha FROM snapshots WHERE id = ?1",
            [snapshot_id],
            |row| row.get(0),
        )
        .map_err(|e| unavailable(&query.spec, e.to_string()))?;

    if snapshot_sha != query.sha {
        return Err(unavailable(
            &query.spec,
            format!(
                "views need the indexed trunk snapshot of {}; \
                 PR previews and stale results have no slice index",
                query.spec
            ),
        ));
    }

    // Step 3: load the slice index for this anchor.
    let index = load_slice_index(conn, snapshot_id, &query.anchor)
        .map_err(|e| unavailable(&query.spec, e.to_string()))?;

    let index = match index {
        Some(idx) => idx,
        None => {
            if !has_slice_rows(conn, snapshot_id)
                .map_err(|e| unavailable(&query.spec, e.to_string()))?
            {
                return Err(unavailable(
                    &query.spec,
                    format!(
                        "{} was indexed without slice data; run `webspec-index update -s {}`",
                        query.spec, query.spec
                    ),
                ));
            } else if query.section_type != "algorithm" {
                return Err(SliceError {
                    code: SliceErrorCode::NotAnAlgorithm,
                    message: format!(
                        "{}#{} is a {} section, not an algorithm",
                        query.spec, query.anchor, query.section_type
                    ),
                    candidates: vec![],
                });
            } else {
                return Err(unavailable(
                    &query.spec,
                    format!("{}#{} has no step structure", query.spec, query.anchor),
                ));
            }
        }
    };

    // Step 4: slice, render, update content.
    let s = slice(&index, &view)?;
    let markdown = query.content.as_deref().unwrap_or("");
    let rendered = render_view(markdown, &index, &s);
    if query.content.is_some() {
        query.content = Some(rendered.content.clone());
    }

    // Step 5: build and return the result.
    Ok(slice_result(
        format!("{}#{}", query.spec, query.anchor),
        &index,
        s,
        &rendered,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::slice::select::ViewRequest;

    const GO: &str = r##"<div class="algorithm"><p>To <dfn id="go">go</dfn> given a <var>foo</var>:</p><ol>
<li><p>Let <var>a</var> be <var>foo</var>.</p></li>
<li><p>Return.</p></li>
<li><p>Set <var>b</var> to <var>a</var>.</p></li></ol></div>
<p>A <dfn id="thing">thing</dfn> is nice.</p>"##;

    fn involving(name: &str) -> ViewRequest {
        ViewRequest {
            involving: vec![name.into()],
            ..Default::default()
        }
    }

    fn query(conn: &rusqlite::Connection, anchor: &str) -> crate::model::QueryResult {
        crate::query_section_from_conn(conn, "HTML", anchor)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn apply_view_replaces_content_and_counts() {
        let conn = crate::state::testing::db_with(&[("HTML", GO)]);
        let mut q = query(&conn, "go");
        let result = apply_view(&conn, &mut q, &involving("foo")).unwrap();
        let content = q.content.unwrap();
        assert!(
            content.contains("- [step 2 omitted: no use of *foo*]\n\n3. "),
            "{content}"
        );
        assert!(content.starts_with("To "), "the intro is kept");
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["algorithm"], "HTML#go");
        assert_eq!(
            json["status"],
            serde_json::json!({
                "semantics": "may", "scope": "algorithm", "rendering": "aligned",
                "counts": {"steps": 3, "kept": 2, "matched": 2, "inherited": 0, "context": 0, "omitted": 1},
                "issues": []
            })
        );
        assert_eq!(
            json["variables"][1],
            serde_json::json!({"name": "a", "basis": "let", "step": "1", "from": ["foo"]})
        );
    }

    #[test]
    fn views_on_snapshots_without_slice_rows_are_unavailable() {
        let conn = crate::state::testing::db_with(&[("HTML", GO)]);
        let mut q = query(&conn, "go");
        q.sha = "hash:some-pr-preview".into();
        assert_eq!(
            apply_view(&conn, &mut q, &involving("foo"))
                .unwrap_err()
                .code,
            SliceErrorCode::Unavailable
        );
        conn.execute("DELETE FROM state_slices", []).unwrap();
        let mut q = query(&conn, "go");
        let e = apply_view(&conn, &mut q, &involving("foo")).unwrap_err();
        assert_eq!(e.code, SliceErrorCode::Unavailable);
        assert!(
            e.message.contains("webspec-index update -s HTML"),
            "{}",
            e.message
        );
        assert!(
            q.content.unwrap().contains("2. Return."),
            "content untouched on error"
        );
    }

    #[test]
    fn non_algorithm_sections_and_bad_views() {
        let conn = crate::state::testing::db_with(&[("HTML", GO)]);
        let mut q = query(&conn, "thing");
        let e = apply_view(&conn, &mut q, &involving("foo")).unwrap_err();
        assert_eq!(e.code, SliceErrorCode::NotAnAlgorithm);
        assert!(e.message.contains("definition"), "{}", e.message);
        let mut q = query(&conn, "go");
        assert_eq!(
            apply_view(&conn, &mut q, &ViewRequest::default())
                .unwrap_err()
                .code,
            SliceErrorCode::InvalidSelector
        );
        assert_eq!(
            apply_view(&conn, &mut q, &involving("nope"))
                .unwrap_err()
                .code,
            SliceErrorCode::UnknownVariable
        );
    }
}
