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
        body_nodes: None,
    })
}

/// Grammar only: unit tests must not change when the bundled catalog grows.
pub fn extract_html(html: &str, spec: &str) -> StateSpec {
    extract_with_catalog(html, spec, &StateCatalog::default())
}

/// The spec §8.3 `add-to-field-collection` verb rule as a state catalog file.
pub const ADD_RULE_YAML: &str = r#"schema: 1
package: webspec-semantics
rules:
  - id: add-to-field-collection
    match:
      text: '(?:^|, then |; )Add (?P<operand>.+?) to (?P<target>(?:the |this )?\S.*?)(?:\.|,|;|$)'
      exclude_text: ['(?i)\badd \S+ to \*\w+\*(?:\.|$)']
    emit:
      kind: state.mutate
      params:
        op: append
        target: {capture: target, from: path}
        operand: {capture: operand, from: text}
"#;

pub const MINI: &str = r##"<pre><code class="idl">partial interface <dfn data-lt="" id="document">Document</dfn> {};</code></pre>
<h3 id="the-document-object">The Document object</h3>
<p>Each <code><a href="#document">Document</a></code> has an <dfn id="is-initial-about:blank">is initial <code>about:blank</code></dfn>, which is a boolean, initially false.</p>
<div data-algorithm=""><p>The <dfn id="document-open-steps">document open steps</dfn>, given a <var>document</var>, are as follows:</p><ol>
<li><p>Set <var>document</var>'s <a href="#is-initial-about:blank">is initial <code>about:blank</code></a> to false.</p></li>
<li><p>If <var>document</var>'s <a href="#is-initial-about:blank">is initial <code>about:blank</code></a> is true, then return.</p></li>
<li><p>Add <var>x</var> to <var>document</var>'s <a href="#is-initial-about:blank">is initial <code>about:blank</code></a>.</p></li></ol></div>"##;

/// A `Document` initializer whose entries are a `<dl>` (spec §12.2 fixture 8).
pub const DL_HTML: &str = r##"<pre><code class="idl">partial interface <dfn data-lt="" id="document">Document</dfn> {};</code></pre>
      <p>Each <code><a href="#document">Document</a></code> has an <dfn id="is-initial-about:blank">is initial <code>about:blank</code></dfn>, which is a boolean, initially false.</p>
      <div data-algorithm=""><p>To <dfn id="creating-a-new-browsing-context">create a new browsing context</dfn>:</p><ol>
      <li><p>Let <var>document</var> be a new <code><a href="#document">Document</a></code>, with:</p>
      <dl class="props"><dt><a href="#is-initial-about:blank">is initial <code>about:blank</code></a></dt><dd>true</dd></dl></li></ol></div>"##;

/// `Event` prose: method, getter and setter steps plus callouts (spec §12.2
/// fixture 13).
pub const PROSE_DOM: &str = r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="event">Event</dfn> {};</pre>
<p>An <dfn data-dfn-type="dfn" id="concept-event">event</dfn>.</p>
<p>Each <a href="#concept-event">event</a> has the following associated flags that are all initially unset:</p>
<ul><li><dfn data-dfn-for="Event" data-dfn-type="dfn" id="stop-propagation-flag">stop propagation flag</dfn></li></ul>
<h3 id="interface-event">Interface Event</h3>
<p>The <dfn data-dfn-for="Event" data-dfn-type="method" id="dom-event-stoppropagation"><code>stopPropagation()</code></dfn> method steps are to set <a href="https://webidl.spec.whatwg.org/#this">this</a>’s <a href="#stop-propagation-flag">stop propagation flag</a>.</p>
<p>The <a href="#dom-event-cancelbubble"><code>cancelBubble</code></a> getter steps are to return true if <a href="https://webidl.spec.whatwg.org/#this">this</a>’s <a href="#stop-propagation-flag">stop propagation flag</a> is set; otherwise false.</p>
<p>The <a href="#dom-event-cancelbubble"><code>cancelBubble</code></a> setter steps are to set <a href="https://webidl.spec.whatwg.org/#this">this</a>’s <a href="#stop-propagation-flag">stop propagation flag</a> if the given value is true; otherwise do nothing.</p>
<div class="note"><p>Set the <a href="#stop-propagation-flag">stop propagation flag</a> only from author code.</p></div>
<dl class="domintro"><dt>x</dt><dd><p>Set the <a href="#stop-propagation-flag">stop propagation flag</a>.</p></dd></dl>"##;

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
        body_nodes: None,
    });
    let spec_id = crate::db::write::insert_or_get_spec(conn, spec, base_url, "offline")?;
    let snapshot = crate::db::write::insert_snapshot(conn, spec_id, &sha, "2026-09-25")?;
    crate::db::write::insert_sections_bulk(conn, snapshot, &parsed.sections)?;
    crate::db::state::store_state(conn, snapshot, &state)?;
    crate::db::state::store_slice_indexes(
        conn,
        snapshot,
        &crate::state::slice::build_slice_indexes(&structure, &state),
    )?;
    Ok(snapshot)
}

pub const QUERY_DOM: &str = r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="interface-node">Node</dfn> {};
interface <dfn data-dfn-type="interface" id="interface-element">Element</dfn> : Node {};
interface <dfn data-dfn-type="interface" id="interface-document">Document</dfn> : Node {};</pre>
<p>A <dfn data-dfn-type="dfn" id="concept-node">node</dfn>. A <dfn data-dfn-type="dfn" id="concept-document">document</dfn>.</p>
<h3 id="interface-node-h">Node</h3>
<p>Each <a href="#concept-node">node</a> has an associated <dfn data-dfn-for="Node" data-dfn-type="dfn" id="concept-node-document">node document</dfn>, set upon creation, that is a <a href="#concept-document">document</a>.</p>
<div class="algorithm"><p>To <dfn id="concept-node-adopt">adopt</dfn> a <var>node</var> into a <var>document</var>:</p><ol>
<li><p>Set <var>node</var>’s <a href="#concept-node-document">node document</a> to <var>document</var>.</p></li></ol></div>"##;

pub const QUERY_HTML: &str = r##"<pre><code class="idl">partial interface <dfn data-lt="" id="document">Document</dfn> {};</code></pre>
<h3 id="the-document-object">The Document object</h3>
<p>Each <code><a href="#document">Document</a></code> has an <dfn id="is-initial-about:blank">is initial <code>about:blank</code></dfn>, which is a boolean, initially false.</p>
<div data-algorithm=""><p>The <dfn id="document-open-steps">document open steps</dfn> are:</p><ol>
<li><p>Set <var>document</var>'s <a href="#is-initial-about:blank">is initial <code>about:blank</code></a> to false.</p></li>
<li><p>Set <var>d</var>'s <a href="https://dom.spec.whatwg.org/#concept-node-document">node document</a> to <var>x</var>.</p></li>
<li><p>Add <var>s</var> to <var>s</var>'s <a href="https://dom.spec.whatwg.org/#concept-node-document">node document</a>'s <a href="#open-dialogs-list">open dialogs list</a>.</p></li></ol></div>
<h3 id="sandboxing">Sandboxing</h3>
<p>A <dfn id="sandboxing-flag-set">sandboxing flag set</dfn> is a set of zero or more of the following flags, which are used to restrict abilities:</p>
<dl><dt>The <dfn id="sandboxed-navigation-browsing-context-flag">sandboxed navigation browsing context flag</dfn></dt><dd><p>This flag prevents content from navigating.</p></dd></dl>
<div data-algorithm=""><p>To <dfn id="parse-a-sandboxing-directive">parse a sandboxing directive</dfn> into <var>output</var>:</p><ol>
<li><p>Set <var>output</var>'s <a href="#sandboxed-navigation-browsing-context-flag">sandboxed navigation browsing context flag</a>.</p></li>
<li><p>Unset <var>output</var>'s <a href="#sandboxed-navigation-browsing-context-flag">sandboxed navigation browsing context flag</a>.</p></li></ol></div>"##;

/// In-memory DB holding the given specs, indexed offline with the grammar only
/// (empty catalog), so query tests do not change when the bundled catalog grows.
pub fn db_with(specs: &[(&str, &str)]) -> rusqlite::Connection {
    let conn = crate::db::open_in_memory().expect("in-memory db");
    for (spec, html) in specs {
        index_offline_with(&conn, spec, base_url(spec), html, &StateCatalog::default())
            .expect("index_offline");
    }
    conn
}

/// Structure, state (empty catalog) and slice indexes of one HTML string.
pub fn slice_indexes_html(html: &str, spec: &str) -> Vec<crate::state::slice::SliceIndex> {
    let structure =
        crate::parse::steps::extract_step_structure(html, spec, base_url(spec), "hash:t");
    let state = extract_html(html, spec);
    crate::state::slice::build_slice_indexes(&structure, &state)
}

/// DOM and HTML query fixtures together.
pub fn query_fixture_db() -> rusqlite::Connection {
    db_with(&[("DOM", QUERY_DOM), ("HTML", QUERY_HTML)])
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

/// Golden-set site key: `SPEC#subject:step` when there is a step path,
/// `SPEC#subject (role)` for a prose role other than `normative`, else `SPEC#subject`.
pub fn site_key(spec: &str, subject: &str, step_path: Option<&str>, role: Option<&str>) -> String {
    match (step_path, role) {
        (Some(step), _) => format!("{spec}#{subject}:{step}"),
        (None, Some(role)) if role != "normative" => format!("{spec}#{subject} ({role})"),
        _ => format!("{spec}#{subject}"),
    }
}

/// Hand-built slice index for query-time tests. `steps`: `(path, mentioned names)` in document
/// order; `edges`: `(step path, kind, defined or root name, used names)`; `loops`: `(step path,
/// loop-bound name)`. Names are interned in first-mention order over steps, then edges, then loops;
/// `mentions` and `uses` are sorted var indexes, as `build_slice_indexes` produces them.
/// `Store` edges get the target `*{name}*'s field`.
pub fn slice_index(
    anchor: &str,
    steps: &[(&str, &[&str])],
    edges: &[(&str, crate::state::slice::DefKind, Option<&str>, &[&str])],
    loops: &[(&str, &str)],
) -> crate::state::slice::SliceIndex {
    use crate::state::slice::index::derive_parents;
    use crate::state::slice::{DefEdge, DefKind, SliceIndex, SliceStep};

    fn intern(vars: &mut Vec<String>, name: &str) -> u32 {
        if let Some(pos) = vars.iter().position(|v| v == name) {
            return pos as u32;
        }
        vars.push(name.to_owned());
        (vars.len() - 1) as u32
    }

    let mut vars: Vec<String> = Vec::new();

    let mut slice_steps: Vec<SliceStep> = Vec::with_capacity(steps.len());
    for (path, names) in steps {
        let mut mentions: Vec<u32> = names.iter().map(|n| intern(&mut vars, n)).collect();
        mentions.sort_unstable();
        mentions.dedup();
        slice_steps.push(SliceStep {
            path: path.to_string(),
            parent: None,
            mentions,
            loop_binds: vec![],
        });
    }

    let paths: Vec<String> = slice_steps.iter().map(|s| s.path.clone()).collect();
    let parents = derive_parents(&paths);
    for (step, parent) in slice_steps.iter_mut().zip(parents) {
        step.parent = parent;
    }

    let step_paths: Vec<&str> = slice_steps.iter().map(|s| s.path.as_str()).collect();
    let find_step = |path: &str| -> usize {
        step_paths
            .iter()
            .position(|p| *p == path)
            .expect("step path not found")
    };

    let mut slice_edges: Vec<DefEdge> = Vec::with_capacity(edges.len());
    for (step_path, kind, name, used_names) in edges {
        let step = find_step(step_path) as u32;
        let var = name.map(|n| intern(&mut vars, n));
        let mut uses: Vec<u32> = used_names.iter().map(|n| intern(&mut vars, n)).collect();
        uses.sort_unstable();
        uses.dedup();
        let target = if *kind == DefKind::Store {
            name.map(|n| format!("*{n}*'s field"))
        } else {
            None
        };
        slice_edges.push(DefEdge {
            step,
            kind: *kind,
            var,
            uses,
            target,
        });
    }

    let loop_assignments: Vec<(usize, &str)> = loops
        .iter()
        .map(|(path, name)| (find_step(path), *name))
        .collect();
    drop(step_paths);
    for (idx, loop_name) in loop_assignments {
        let var_idx = intern(&mut vars, loop_name);
        slice_steps[idx].loop_binds.push(var_idx);
    }

    SliceIndex {
        anchor: anchor.to_owned(),
        vars,
        steps: slice_steps,
        edges: slice_edges,
    }
}

/// A reduced DOM "insert": 18 steps, Let chains, a store and a loop (spec §12 fixture G1).
pub fn slice_fixture_insert() -> crate::state::slice::SliceIndex {
    slice_index(
        "insert",
        &[
            ("1", &["nodes", "node"]),
            ("2", &["count", "nodes"]),
            ("3", &["count"]),
            ("4", &["node"]),
            ("4.1", &[]),
            ("4.2", &["node", "nodes"]),
            ("5", &["child"]),
            ("5.1", &["parent", "child", "count"]),
            ("5.2", &["parent", "child", "count"]),
            ("6", &["previousSibling", "child", "parent"]),
            ("7", &["node", "nodes"]),
            ("7.1", &["node", "parent"]),
            ("7.2", &["node", "parent"]),
            ("7.3", &["node", "inclusiveDescendant"]),
            ("7.3.1", &["inclusiveDescendant"]),
            (
                "8",
                &[
                    "suppressObservers",
                    "parent",
                    "nodes",
                    "previousSibling",
                    "child",
                ],
            ),
            ("9", &["parent"]),
            ("10", &["staticNodeList"]),
        ],
        &[
            (
                "1",
                crate::state::slice::DefKind::Let,
                Some("nodes"),
                &["node"],
            ),
            (
                "2",
                crate::state::slice::DefKind::Let,
                Some("count"),
                &["nodes"],
            ),
            (
                "6",
                crate::state::slice::DefKind::Let,
                Some("previousSibling"),
                &["child", "parent"],
            ),
            (
                "7.2",
                crate::state::slice::DefKind::Store,
                Some("parent"),
                &["node"],
            ),
        ],
        &[("7.3", "inclusiveDescendant")],
    )
}

/// A reduced navigate: 11 steps, backward-slice fixture (spec §12 fixture G3).
pub fn slice_fixture_navigate() -> crate::state::slice::SliceIndex {
    slice_index(
        "navigate",
        &[
            ("1", &["documentResource"]),
            ("2", &["initiator", "sourceDocument"]),
            ("3", &["sourceDocument"]),
            ("3.1", &["initiator", "navigable"]),
            ("4", &["state", "referrerPolicy", "initiator"]),
            ("4.1", &["state", "initiator"]),
            ("5", &["entry", "url", "state"]),
            ("6", &["navigable"]),
            ("7", &["navigable", "entry", "historyHandling"]),
            ("8", &["entry"]),
            ("9", &["other"]),
        ],
        &[
            (
                "2",
                crate::state::slice::DefKind::Let,
                Some("initiator"),
                &["sourceDocument"],
            ),
            (
                "3.1",
                crate::state::slice::DefKind::Set,
                Some("initiator"),
                &["navigable"],
            ),
            (
                "4",
                crate::state::slice::DefKind::Let,
                Some("state"),
                &["referrerPolicy", "initiator"],
            ),
            (
                "4",
                crate::state::slice::DefKind::Opaque,
                None,
                &["initiator"],
            ),
            (
                "4.1",
                crate::state::slice::DefKind::Store,
                Some("state"),
                &["initiator"],
            ),
            (
                "5",
                crate::state::slice::DefKind::Let,
                Some("entry"),
                &["url", "state"],
            ),
            (
                "6",
                crate::state::slice::DefKind::Store,
                Some("navigable"),
                &[],
            ),
            ("8", crate::state::slice::DefKind::Set, Some("entry"), &[]),
        ],
        &[],
    )
}

/// Classes (snake_case) of the occurrences of `field_selector` (`SPEC#anchor`) in the
/// sources whose [`site_key`] is `site`, across every current snapshot's state model.
pub fn occurrence_classes(
    conn: &rusqlite::Connection,
    field_selector: &str,
    site: &str,
) -> std::collections::BTreeSet<String> {
    use crate::state::extract::{class_name, role_name};
    use crate::state::ir::SourceContext;
    let (field_spec, field_anchor) = field_selector
        .split_once('#')
        .expect("field selector is SPEC#anchor");
    let mut classes = std::collections::BTreeSet::new();
    for snapshot in crate::db::state::state_snapshots(conn).expect("state snapshots") {
        let Some(state) =
            crate::db::state::load_state_model(conn, snapshot.id).expect("state model")
        else {
            continue;
        };
        let sources: std::collections::HashSet<&str> = state
            .sources
            .iter()
            .filter(|source| {
                let (step_path, role) = match &source.context {
                    SourceContext::Algorithm { step_path, .. } => (step_path.as_deref(), None),
                    SourceContext::BranchLabel { step_path, .. } => {
                        (Some(step_path.as_str()), None)
                    }
                    SourceContext::Prose {
                        step_path, role, ..
                    } => (step_path.as_deref(), Some(role_name(*role))),
                };
                site_key(&state.spec, &source.subject.anchor, step_path, role) == site
            })
            .map(|source| source.id.as_str())
            .collect();
        for occurrence in &state.occurrences {
            let targets_field = occurrence
                .target
                .as_ref()
                .is_some_and(|t| t.spec == field_spec && t.anchor == field_anchor);
            if targets_field && sources.contains(occurrence.source_id.as_str()) {
                classes.insert(class_name(occurrence.class).to_owned());
            }
        }
    }
    classes
}
