//! Offline helpers for tests and examples: extract (and later index) spec HTML
//! without network, fetch state or update checks.
use crate::state::{extract_state, StateCatalog, StateInputs, StateSpec};

pub fn base_url(spec: &str) -> &'static str {
    match spec {
        "DOM" => "https://dom.spec.whatwg.org/",
        "ECMA-262" => "https://tc39.es/ecma262/",
        _ => "https://html.spec.whatwg.org/",
    }
}

pub fn extract_with_catalog(html: &str, spec: &str, catalog: &StateCatalog) -> StateSpec {
    let base = base_url(spec);
    let document = scraper::Html::parse_document(html);
    let parsed = crate::parse::parse_spec(html, spec, base).expect("parse_spec");
    let structure =
        crate::parse::steps::extract_step_structure_from_document(&document, spec, base, "hash:t");
    extract_state(&StateInputs {
        document: &document,
        spec,
        base_url: base,
        snapshot_sha: "hash:t",
        structure: &structure,
        sections: &parsed.sections,
        idl_definitions: &parsed.idl_definitions,
        catalog,
    })
}

/// Grammar only: unit tests must not change when the bundled catalog grows.
pub fn extract_html(html: &str, spec: &str) -> StateSpec {
    extract_with_catalog(html, spec, &StateCatalog::default())
}

pub const MINI: &str = r##"<pre><code class="idl">partial interface <dfn data-lt="" id="document">Document</dfn> {};</code></pre>
<h3 id="the-document-object">The Document object</h3>
<p>Each <code><a href="#document">Document</a></code> has an <dfn id="is-initial-about:blank">is initial <code>about:blank</code></dfn>, which is a boolean, initially false.</p>
<div data-algorithm=""><p>The <dfn id="document-open-steps">document open steps</dfn>, given a <var>document</var>, are as follows:</p><ol>
<li><p>Set <var>document</var>'s <a href="#is-initial-about:blank">is initial <code>about:blank</code></a> to false.</p></li>
<li><p>If <var>document</var>'s <a href="#is-initial-about:blank">is initial <code>about:blank</code></a> is true, then return.</p></li>
<li><p>Add <var>x</var> to <var>document</var>'s <a href="#is-initial-about:blank">is initial <code>about:blank</code></a>.</p></li></ol></div>"##;

/// Parse, extract and store one spec snapshot without network or fetch state.
pub fn index_offline_with(
    conn: &rusqlite::Connection,
    spec: &str,
    base_url: &str,
    html: &str,
    catalog: &StateCatalog,
) -> anyhow::Result<i64> {
    let parsed = crate::parse::parse_spec(html, spec, base_url)?;
    let document = scraper::Html::parse_document(html);
    let sha = format!("hash:{spec}-offline");
    let structure =
        crate::parse::steps::extract_step_structure_from_document(&document, spec, base_url, &sha);
    let state = extract_state(&StateInputs {
        document: &document,
        spec,
        base_url,
        snapshot_sha: &sha,
        structure: &structure,
        sections: &parsed.sections,
        idl_definitions: &parsed.idl_definitions,
        catalog,
    });
    let spec_id = crate::db::write::insert_or_get_spec(conn, spec, base_url, "offline")?;
    let snapshot = crate::db::write::insert_snapshot(conn, spec_id, &sha, "2026-09-25")?;
    crate::db::write::insert_sections_bulk(conn, snapshot, &parsed.sections)?;
    crate::db::state::store_state(conn, snapshot, &state)?;
    Ok(snapshot)
}

/// Parse, extract and store one spec snapshot using the bundled catalog.
pub fn index_offline(
    conn: &rusqlite::Connection,
    spec: &str,
    base_url: &str,
    html: &str,
) -> anyhow::Result<i64> {
    index_offline_with(
        conn,
        spec,
        base_url,
        html,
        crate::state::extract::bundled_catalog(),
    )
}
