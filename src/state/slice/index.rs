use crate::parse::steps::{
    InlineToken, InlineTokenKind, LinkSpan, StructuralAlgorithm, StructuralBranch,
    StructuralSegment, StructuralSpec,
};
use crate::state::ir::{
    Expr, Hop, Path, Root, SourceContext, Statement, StatementKind, StatementSource,
};
use crate::state::model::Literal;
use crate::state::StateSpec;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceIndex {
    pub anchor: String,
    pub vars: Vec<String>,
    pub steps: Vec<SliceStep>,
    pub edges: Vec<DefEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceStep {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub mentions: Vec<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub loop_binds: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DefEdge {
    pub step: u32,
    pub kind: DefKind,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub var: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub uses: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefKind {
    Let,
    Set,
    Mutate,
    Store,
    Opaque,
}

impl SliceIndex {
    pub fn step_by_path(&self, path: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.path == path)
    }

    pub fn children(&self, parent: Option<usize>) -> Vec<usize> {
        let parent = parent.map(|p| p as u32);
        (0..self.steps.len())
            .filter(|&i| self.steps[i].parent == parent)
            .collect()
    }

    pub fn depth(&self, step: usize) -> usize {
        self.steps[step].path.split('.').count()
    }

    pub fn var(&self, name: &str) -> Option<u32> {
        self.vars.iter().position(|v| v == name).map(|i| i as u32)
    }

    pub fn name(&self, var: u32) -> &str {
        &self.vars[var as usize]
    }
}

/// Spec-wide lookups shared by every algorithm's [`build_one`].
struct Lookups<'s> {
    /// `Init` statements by id, for `Expr::New { init: Some(id) }`.
    inits: BTreeMap<&'s str, &'s Statement>,
}

/// A statement of one algorithm: the index of its step, the statement, and its source.
struct Placed<'s> {
    step: usize,
    statement: &'s Statement,
    source: &'s StatementSource,
}

/// One slice index per structural algorithm, in document order; an algorithm whose anchor an
/// earlier algorithm already has gets no index.
pub fn build_slice_indexes(structure: &StructuralSpec, state: &StateSpec) -> Vec<SliceIndex> {
    let mut seen: HashSet<&str> = HashSet::new();
    let kept: Vec<&StructuralAlgorithm> = structure
        .algorithms
        .iter()
        .filter(|algorithm| seen.insert(algorithm.source.section_anchor.as_str()))
        .collect();
    let mut step_at: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (a, algorithm) in kept.iter().enumerate() {
        for (s, step) in algorithm.steps.iter().enumerate() {
            step_at.insert(step.source.node_id.as_str(), (a, s));
        }
    }
    let sources: BTreeMap<&str, &StatementSource> = state
        .sources
        .iter()
        .map(|source| (source.id.as_str(), source))
        .collect();
    let lookups = Lookups {
        inits: state
            .statements
            .iter()
            .filter(|statement| matches!(statement.kind, StatementKind::Init { .. }))
            .map(|statement| (statement.id.as_str(), statement))
            .collect(),
    };
    let mut buckets: Vec<Vec<Placed>> = (0..kept.len()).map(|_| Vec::new()).collect();
    for statement in &state.statements {
        let Some(&source) = sources.get(statement.source_id.as_str()) else {
            continue;
        };
        let step_id = match &source.context {
            SourceContext::Algorithm {
                step_id: Some(id), ..
            }
            | SourceContext::BranchLabel { step_id: id, .. } => id.as_str(),
            _ => continue,
        };
        if let Some(&(a, step)) = step_at.get(step_id) {
            buckets[a].push(Placed {
                step,
                statement,
                source,
            });
        }
    }
    kept.iter()
        .zip(&buckets)
        .map(|(algorithm, statements)| build_one(algorithm, statements, &lookups))
        .collect()
}

fn build_one<'a, 's>(
    algorithm: &'a StructuralAlgorithm,
    statements: &[Placed<'s>],
    lookups: &Lookups<'s>,
) -> SliceIndex {
    let step_of: HashMap<&str, usize> = algorithm
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| (step.source.node_id.as_str(), i))
        .collect();
    let mut segments: Vec<Vec<&StructuralSegment>> = vec![Vec::new(); algorithm.steps.len()];
    for segment in &algorithm.segments {
        if let Some(&i) = segment
            .owner_step_id
            .as_deref()
            .and_then(|id| step_of.get(id))
        {
            segments[i].push(segment);
        }
    }
    let mut branches: Vec<Vec<&StructuralBranch>> = vec![Vec::new(); algorithm.steps.len()];
    for branch in &algorithm.branches {
        if let Some(&i) = step_of.get(branch.parent_step_id.as_str()) {
            branches[i].push(branch);
        }
    }

    let paths: Vec<String> = algorithm
        .steps
        .iter()
        .map(|step| {
            step.path
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect();
    let parents = derive_parents(&paths);

    let mut vars: Vec<String> = Vec::new();
    let mut var_of: HashMap<&str, u32> = HashMap::new();
    let mut intern = |name: &'a str| -> u32 {
        *var_of.entry(name).or_insert_with(|| {
            vars.push(name.to_owned());
            (vars.len() - 1) as u32
        })
    };
    let mut steps = Vec::with_capacity(paths.len());
    for ((path, parent), (segments, branches)) in paths
        .into_iter()
        .zip(parents)
        .zip(segments.iter().zip(&branches))
    {
        let mut mentions = Vec::new();
        let mut loop_binds = Vec::new();
        for &segment in segments {
            let mut bound_clauses: Vec<usize> = Vec::new();
            for token in mentioned(&segment.tokens, &segment.links) {
                let v = intern(&token.source_text);
                mentions.push(v);
                if let Some(clause) = loop_clause(&segment.text, token) {
                    if !bound_clauses.contains(&clause) {
                        bound_clauses.push(clause);
                        loop_binds.push(v);
                    }
                }
            }
        }
        for &branch in branches {
            for token in mentioned(&branch.label_tokens, &branch.label_links) {
                mentions.push(intern(&token.source_text));
            }
        }
        mentions.sort_unstable();
        mentions.dedup();
        steps.push(SliceStep {
            path,
            parent,
            mentions,
            loop_binds,
        });
    }

    let known = |name: &str| var_of.get(name).copied();
    let uses_of = |names: Vec<&str>| -> Vec<u32> {
        let mut uses: Vec<u32> = names.into_iter().filter_map(known).collect();
        uses.sort_unstable();
        uses.dedup();
        uses
    };
    // A `<dl>` initializer is one `Init` per `<dt>`, not linked from the `new` it initializes;
    // its entry values are uses of the statement in the introducing segment.
    let mut dl_entries: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for placed in statements {
        if let (
            StatementKind::Init { entries, .. },
            SourceContext::BranchLabel { segment_id, .. },
        ) = (&placed.statement.kind, &placed.source.context)
        {
            let names = dl_entries.entry(segment_id).or_default();
            for entry in entries {
                expr_vars(&entry.value, lookups, names);
            }
        }
    }
    let value_vars = |value: &'s Expr, source: &StatementSource| -> Vec<&'s str> {
        let mut names = Vec::new();
        expr_vars(value, lookups, &mut names);
        if let (Expr::New { init: None, .. }, Some(entries)) =
            (value, dl_entries.get(source.id.as_str()))
        {
            names.extend(entries);
        }
        names
    };
    let mut edges = Vec::new();
    for placed in statements {
        let step = placed.step as u32;
        let source = placed.source;
        let edge = |kind, var: Option<&str>, uses, target| DefEdge {
            step,
            kind,
            var: var.and_then(known),
            uses,
            target,
        };
        let store = |target: &'s Path, mut names: Vec<&'s str>| {
            if let Some(subscript) = &target.subscript {
                expr_vars(subscript, lookups, &mut names);
            }
            edge(
                DefKind::Store,
                root_var(target),
                uses_of(names),
                Some(render_path(target, source)),
            )
        };
        match &placed.statement.kind {
            StatementKind::Let { var, value } => {
                let names = value_vars(value, source);
                edges.push(edge(DefKind::Let, Some(var), uses_of(names), None));
            }
            StatementKind::Set { targets, value, .. } => {
                let value_names = value_vars(value, source);
                for target in targets {
                    let names = value_names.clone();
                    edges.push(
                        match (&target.root, target.hops.is_empty(), &target.subscript) {
                            (Root::Var(x), true, None) => {
                                edge(DefKind::Set, Some(x), uses_of(names), None)
                            }
                            _ => store(target, names),
                        },
                    );
                }
            }
            StatementKind::Mutate {
                target, operand, ..
            } => {
                let mut names = Vec::new();
                if let Some(operand) = operand {
                    expr_vars(operand, lookups, &mut names);
                }
                edges.push(match (&target.root, target.hops.is_empty()) {
                    (Root::Var(x), true) => {
                        if let Some(subscript) = &target.subscript {
                            expr_vars(subscript, lookups, &mut names);
                        }
                        edge(DefKind::Mutate, Some(x), uses_of(names), None)
                    }
                    _ => store(target, names),
                });
            }
            StatementKind::Init { .. } => {}
            StatementKind::Opaque { .. } => {
                let span = &placed.statement.span;
                let names = mentioned(&source.tokens, &source.links)
                    .filter(|token| span.start <= token.span.start && token.span.end <= span.end)
                    .map(|token| token.source_text.as_str())
                    .collect();
                edges.push(edge(DefKind::Opaque, None, uses_of(names), None));
            }
        }
    }

    SliceIndex {
        anchor: algorithm.source.section_anchor.clone(),
        vars,
        steps,
        edges,
    }
}

fn root_var(path: &Path) -> Option<&str> {
    match &path.root {
        Root::Var(x) => Some(x),
        _ => None,
    }
}

/// Variable names `expr` reads, appended to `out` (unfiltered, possibly repeated).
fn expr_vars<'s>(expr: &'s Expr, lookups: &Lookups<'s>, out: &mut Vec<&'s str>) {
    match expr {
        Expr::Var(name) => out.push(name),
        Expr::Path(path) => {
            match &path.root {
                Root::Var(name) => out.push(name),
                Root::Opaque { text } => out.extend(scan_starred(text)),
                Root::This | Root::Link { .. } | Root::Implicit => {}
            }
            if let Some(subscript) = &path.subscript {
                expr_vars(subscript, lookups, out);
            }
        }
        Expr::New { init: Some(id), .. } => {
            if let Some(Statement {
                kind: StatementKind::Init { entries, .. },
                ..
            }) = lookups.inits.get(id.as_str()).copied()
            {
                for entry in entries {
                    expr_vars(&entry.value, lookups, out);
                }
            }
        }
        Expr::Opaque { text } => out.extend(scan_starred(text)),
        Expr::This | Expr::Literal(_) | Expr::New { init: None, .. } => {}
    }
}

/// Names between each successive pair of `*`s: `*a* and *b*` yields `a`, `b`.
fn scan_starred(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || loop {
        let open = rest.find('*')?;
        let after = &rest[open + 1..];
        let close = after.find('*')?;
        let name = &after[..close];
        rest = &after[close + 1..];
        if !name.is_empty() {
            return Some(name);
        }
    })
}

/// The store target as prose: `*d*'s f`, `this's [[slot]]`, `*m*[*k*]`. The possessive follows
/// the source's typography.
fn render_path(path: &Path, source: &StatementSource) -> String {
    let possessive = if source.text.contains("’s") {
        "’s "
    } else {
        "'s "
    };
    let mut parts: Vec<String> = Vec::with_capacity(path.hops.len() + 1);
    match &path.root {
        Root::Var(x) => parts.push(format!("*{x}*")),
        Root::This => parts.push("this".to_owned()),
        Root::Link { link_id, .. } => {
            if let Some(link) = source.links.iter().find(|link| &link.id == link_id) {
                parts.push(link.visible_text.clone());
            }
        }
        Root::Opaque { text } => parts.push(text.clone()),
        Root::Implicit => {}
    }
    for hop in &path.hops {
        parts.push(match hop {
            Hop::Field { visible_text, .. } => visible_text.clone(),
            Hop::CodeMember { name } => format!("`{name}`"),
            Hop::Slot { name } => format!("[[{name}]]"),
        });
    }
    let mut rendered = parts.join(possessive);
    if let Some(subscript) = &path.subscript {
        rendered.push('[');
        rendered.push_str(&subscript_text(subscript, source));
        rendered.push(']');
    }
    rendered
}

fn subscript_text(expr: &Expr, source: &StatementSource) -> String {
    match expr {
        Expr::Var(n) => format!("*{n}*"),
        Expr::Literal(literal) => match literal {
            Literal::Bool(b) => b.to_string(),
            Literal::Null => "null".to_owned(),
            Literal::Undefined => "undefined".to_owned(),
            Literal::Number(n) => n.clone(),
            Literal::String(s) => format!("\"{s}\""),
        },
        Expr::Opaque { text } => text.clone(),
        Expr::Path(path) => render_path(path, source),
        Expr::This => "this".to_owned(),
        Expr::New { .. } => "a new value".to_owned(),
    }
}

/// Variable tokens that mention their variable: a token whose span equals a link span is the
/// label of a named argument (`<a><var>x</var></a>`, `<var><a>x</a></var>`).
fn mentioned<'a>(
    tokens: &'a [InlineToken],
    links: &'a [LinkSpan],
) -> impl Iterator<Item = &'a InlineToken> {
    tokens.iter().filter(move |token| {
        token.kind == InlineTokenKind::Variable && links.iter().all(|link| link.span != token.span)
    })
}

/// Start of the `For each` clause that `token` is the loop variable of: the token is followed by
/// ` of `, ` in ` or ` from `, and the text from the clause start up to it begins with `For each `.
fn loop_clause(text: &str, token: &InlineToken) -> Option<usize> {
    let after = text.get(token.span.end..)?;
    if ![" of ", " in ", " from "]
        .iter()
        .any(|word| after.starts_with(word))
    {
        return None;
    }
    let before = text.get(..token.span.start)?;
    let start = [". ", "; ", ": ", ", "]
        .iter()
        .filter_map(|sep| before.rfind(sep).map(|at| at + sep.len()))
        .max()
        .unwrap_or(0);
    let clause = &before[start..];
    (clause.starts_with("For each ") || clause.starts_with("for each ")).then_some(start)
}

/// Parent of each step: the nearest preceding step whose path is this path minus its last
/// component. Paths, not `parent_step_id`: steps of nested bodies have no structural parent.
pub(crate) fn derive_parents(paths: &[String]) -> Vec<Option<u32>> {
    let mut last_at: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let mut parents = Vec::with_capacity(paths.len());
    for (i, path) in paths.iter().enumerate() {
        parents.push(
            path.rsplit_once('.')
                .and_then(|(prefix, _)| last_at.get(prefix).copied()),
        );
        last_at.insert(path.as_str(), i as u32);
    }
    parents
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::{slice_index, slice_indexes_html};

    #[test]
    fn slice_index_json_is_compact_and_round_trips() {
        let index = SliceIndex {
            anchor: "go".into(),
            vars: vec!["foo".into(), "a".into()],
            steps: vec![
                SliceStep {
                    path: "1".into(),
                    parent: None,
                    mentions: vec![0, 1],
                    loop_binds: vec![],
                },
                SliceStep {
                    path: "1.1".into(),
                    parent: Some(0),
                    mentions: vec![],
                    loop_binds: vec![1],
                },
            ],
            edges: vec![DefEdge {
                step: 0,
                kind: DefKind::Let,
                var: Some(1),
                uses: vec![0],
                target: None,
            }],
        };
        let json = serde_json::to_string(&index).unwrap();
        assert_eq!(
            json,
            r#"{"anchor":"go","vars":["foo","a"],"steps":[{"path":"1","mentions":[0,1]},{"path":"1.1","parent":0,"loop_binds":[1]}],"edges":[{"step":0,"kind":"let","var":1,"uses":[0]}]}"#
        );
        assert_eq!(serde_json::from_str::<SliceIndex>(&json).unwrap(), index);
    }

    #[test]
    fn builder_derives_parents_from_paths_and_interns_in_mention_order() {
        let index = slice_index(
            "go",
            &[
                ("1", &["a", "foo"]),
                ("2", &[]),
                ("2.1", &["b"]),
                ("2.1.1", &["a"]),
                ("3", &["foo"]),
            ],
            &[("2.1", DefKind::Let, Some("b"), &["a"])],
            &[("3", "item")],
        );
        assert_eq!(index.vars, ["a", "foo", "b", "item"]);
        let parents: Vec<Option<u32>> = index.steps.iter().map(|s| s.parent).collect();
        assert_eq!(parents, [None, None, Some(1), Some(2), None]);
        assert_eq!(index.step_by_path("2.1"), Some(2));
        assert_eq!(index.step_by_path("9"), None);
        assert_eq!(index.children(None), [0, 1, 4]);
        assert_eq!(index.children(Some(1)), [2]);
        assert_eq!(index.steps[4].loop_binds, [3]);
        assert_eq!(index.depth(3), 3);
        assert_eq!(index.var("b"), Some(2));
        assert_eq!(index.name(3), "item");
        assert_eq!(
            index.edges[0],
            DefEdge {
                step: 2,
                kind: DefKind::Let,
                var: Some(2),
                uses: vec![0],
                target: None
            }
        );
    }

    #[test]
    fn duplicate_paths_take_the_nearest_preceding_prefix_as_parent() {
        let paths: Vec<String> = ["1", "1.1", "1", "1.1", "2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(derive_parents(&paths), [None, Some(0), None, Some(2), None]);
    }

    #[test]
    fn store_edges_from_the_builder_carry_a_target() {
        let index = slice_index(
            "go",
            &[("1", &["d", "foo"])],
            &[("1", DefKind::Store, Some("d"), &["foo"])],
            &[],
        );
        assert_eq!(index.edges[0].target.as_deref(), Some("*d*'s field"));
    }

    fn one(html: &str) -> SliceIndex {
        let mut all = slice_indexes_html(html, "HTML");
        assert_eq!(all.len(), 1, "{all:?}");
        all.remove(0)
    }

    fn names(index: &SliceIndex, step: &str) -> Vec<String> {
        let s = &index.steps[index.step_by_path(step).unwrap()];
        s.mentions
            .iter()
            .map(|v| index.name(*v).to_string())
            .collect()
    }

    const MENTIONS: &str = r##"<div class="algorithm"><p>To <dfn id="go">go</dfn> given a <var>foo</var>:</p><ol>
<li><p>Let <var>a</var> be <var>foo</var>'s <a href="#f">f</a>.</p><p class="note">Note: <var>zzz</var> is only a note.</p></li>
<li><p><a href="#rm">Remove</a> <var>foo</var> with <a href="#rm-so"><var>suppressObservers</var></a> set to true and <var><a href="#rm-x">extra</a></var> set to false.</p></li>
<li><p>Let <var>n</var> be the <a href="#days">number of days in month <var>m</var></a>.</p></li>
<li><p>Switch on <var>a</var>:</p><dl class="switch"><dt><var>mode</var> is "<code>x</code>"</dt><dd><p>Return <var>foo</var>.</p></dd></dl></li>
<li><p><a href="#in-parallel">In parallel</a>, run these steps:</p><ol><li><p>Let <var>b</var> be <var>a</var>.</p></li></ol></li>
<li><p>For each <var>item</var> of <var>foo</var>:</p><ol><li><p>Return.</p></li></ol></li>
</ol></div>
<p><dfn id="f">f</dfn> <dfn id="rm">remove</dfn> <dfn id="rm-so">so</dfn> <dfn id="rm-x">x</dfn> <dfn id="days">days</dfn> <dfn id="in-parallel">in parallel</dfn></p>"##;

    #[test]
    fn mentions_exclude_notes_intro_and_named_argument_labels() {
        let index = one(MENTIONS);
        assert_eq!(index.anchor, "go");
        assert_eq!(index.vars, ["a", "foo", "n", "m", "mode", "b", "item"]);
        assert_eq!(names(&index, "1"), ["a", "foo"]);
        assert_eq!(
            names(&index, "2"),
            ["foo"],
            "labels <a><var> and <var><a> are not mentions"
        );
        assert_eq!(
            names(&index, "3"),
            ["n", "m"],
            "a var inside longer link text is a mention"
        );
        assert_eq!(
            names(&index, "4"),
            ["a", "foo", "mode"],
            "branch labels and branch items are own text"
        );
        assert!(names(&index, "6.1").is_empty());
    }

    #[test]
    fn steps_in_nested_bodies_get_their_parent_from_the_path() {
        let index = one(MENTIONS);
        let structure = crate::parse::steps::extract_step_structure(
            MENTIONS,
            "HTML",
            "https://html.spec.whatwg.org/",
            "hash:t",
        );
        let nested = structure.algorithms[0]
            .steps
            .iter()
            .find(|s| s.path == [5, 1])
            .unwrap();
        assert_eq!(
            nested.parent_step_id, None,
            "the structural IR gives body steps no parent"
        );
        let i = index.step_by_path("5.1").unwrap();
        assert_eq!(
            index.steps[i].parent,
            Some(index.step_by_path("5").unwrap() as u32)
        );
        assert_eq!(
            names(&index, "5.1"),
            ["a", "b"],
            "mentions are sorted var indexes"
        );
    }

    #[test]
    fn loop_markers_record_the_bound_variable() {
        let index = one(MENTIONS);
        let i = index.step_by_path("6").unwrap();
        assert_eq!(index.steps[i].loop_binds, [index.var("item").unwrap()]);
        assert!(index.steps[index.step_by_path("1").unwrap()]
            .loop_binds
            .is_empty());
    }

    #[test]
    fn duplicate_anchors_and_paths() {
        let html = r##"<div class="algorithm"><p>To <dfn id="go">go</dfn>:</p><ol><li><p>Let <var>a</var> be 1.</p></li></ol></div>
<div class="algorithm"><p>To <dfn id="other">other</dfn>:</p><ol><li><p>Let <var>b</var> be 2.</p></li></ol></div>"##;
        let mut structure = crate::parse::steps::extract_step_structure(
            html,
            "HTML",
            "https://html.spec.whatwg.org/",
            "hash:t",
        );
        assert_eq!(structure.algorithms.len(), 2);
        structure.algorithms[1].source.section_anchor = "go".into();
        let state = crate::state::testing::extract_html(html, "HTML");
        let indexes = build_slice_indexes(&structure, &state);
        assert_eq!(indexes.len(), 1, "first algorithm per anchor wins");
        assert_eq!(indexes[0].vars, ["a"]);
    }

    const EDGES: &str = r##"<div class="algorithm"><p>To <dfn id="go">go</dfn> given a <var>foo</var>, a <var>bar</var> and a <var>list</var>:</p><ol>
<li><p>Let <var>a</var> be <var>foo</var>'s <a href="#f">f</a>.</p></li>
<li><p>Let <var>b</var> be <var>a</var>.</p></li>
<li><p>If <var>b</var> is null, then:</p><ol><li><p>Return.</p></li><li><p>Set <var>bar</var> to 1.</p></li></ol></li>
<li><p><a href="https://infra.spec.whatwg.org/#list-append">Append</a> <var>foo</var> to <var>list</var>.</p></li>
<li><p>Set <var>d</var>'s <a href="#f">f</a> to <var>foo</var>.</p></li>
<li><p>Set <var>foo</var> to 1.</p></li>
<li><p>Set <var>m</var>[<var>k</var>] to <var>bar</var>.</p></li>
<li><p>Let <var>t</var> be a new <a href="#thing">thing</a> whose <a href="#f">f</a> is <var>foo</var>.</p></li>
<li><p>Append <var>foo</var> to <var>queue</var>'s <a href="#jobs">jobs</a>.</p></li>
<li><p>Return <var>bar</var> and <var>list</var>.</p></li>
</ol></div>
<p><dfn id="f">f</dfn> <dfn id="thing">thing</dfn> <dfn id="jobs">jobs</dfn></p>"##;

    fn edge(index: &SliceIndex, step: &str, kind: DefKind) -> DefEdge {
        let s = index.step_by_path(step).unwrap() as u32;
        index
            .edges
            .iter()
            .find(|e| e.step == s && e.kind == kind)
            .cloned()
            .unwrap_or_else(|| panic!("no {kind:?} edge at {step}: {:?}", index.edges))
    }

    fn used(index: &SliceIndex, e: &DefEdge) -> Vec<String> {
        e.uses.iter().map(|v| index.name(*v).to_string()).collect()
    }

    #[test]
    fn let_set_mutate_store_and_opaque_edges() {
        let index = one(EDGES);
        let e = edge(&index, "1", DefKind::Let);
        assert_eq!(
            (index.name(e.var.unwrap()), used(&index, &e)),
            ("a", vec!["foo".to_string()])
        );
        let e = edge(&index, "2", DefKind::Let);
        assert_eq!(
            (index.name(e.var.unwrap()), used(&index, &e)),
            ("b", vec!["a".to_string()])
        );
        let e = edge(&index, "3.2", DefKind::Set);
        assert_eq!((index.name(e.var.unwrap()), e.uses.len()), ("bar", 0));
        let e = edge(&index, "4", DefKind::Mutate);
        assert_eq!(
            (index.name(e.var.unwrap()), used(&index, &e)),
            ("list", vec!["foo".to_string()])
        );
        let e = edge(&index, "5", DefKind::Store);
        assert_eq!(index.name(e.var.unwrap()), "d");
        assert_eq!(used(&index, &e), ["foo"]);
        assert_eq!(e.target.as_deref(), Some("*d*'s f"));
        let e = edge(&index, "6", DefKind::Set);
        assert_eq!(index.name(e.var.unwrap()), "foo");
        let e = edge(&index, "7", DefKind::Mutate);
        assert_eq!(index.name(e.var.unwrap()), "m");
        let mut u = used(&index, &e);
        u.sort();
        assert_eq!(u, ["bar", "k"], "map-set operand and subscript are uses");
        let e = edge(&index, "8", DefKind::Let);
        assert_eq!(
            (index.name(e.var.unwrap()), used(&index, &e)),
            ("t", vec!["foo".to_string()]),
            "initializer entries are uses"
        );
        let e = edge(&index, "9", DefKind::Opaque);
        assert_eq!(e.var, None);
        let mut u = used(&index, &e);
        u.sort();
        assert_eq!(u, ["foo", "queue"]);
        assert!(index
            .edges
            .iter()
            .all(|e| e.step != index.step_by_path("10").unwrap() as u32));
    }

    #[test]
    fn dl_initializer_entries_are_uses_of_the_let() {
        let index = one(
            r##"<div class="algorithm"><p>To <dfn id="go">go</dfn>:</p><ol>
<li><p>Let <var>state</var> be a new <a href="#ds">document state</a>, with</p><dl><dt><a href="#ds-origin">origin</a></dt><dd><var>foo</var></dd><dt><a href="#ds-ref">referrer</a></dt><dd><var>bar</var>'s <a href="#f">f</a></dd></dl></li>
<li><p>Return <var>state</var>.</p></li></ol></div>
<p><dfn id="ds">document state</dfn> <dfn id="ds-origin">origin</dfn> <dfn id="ds-ref">referrer</dfn> <dfn id="f">f</dfn></p>"##,
        );
        let e = edge(&index, "1", DefKind::Let);
        assert_eq!(index.name(e.var.unwrap()), "state");
        let mut u = used(&index, &e);
        u.sort();
        assert_eq!(u, ["bar", "foo"]);
    }

    #[test]
    fn typographic_possessive_is_kept_in_store_targets() {
        let index = one(
            r##"<div class="algorithm"><p>To <dfn id="go">go</dfn>:</p><ol><li><p><a href="https://infra.spec.whatwg.org/#set-append">Append</a> <var>node</var> to <var>parent</var>’s <a href="#kids">children</a>.</p></li></ol></div><p><dfn id="kids">children</dfn></p>"##,
        );
        let e = edge(&index, "1", DefKind::Store);
        assert_eq!(e.target.as_deref(), Some("*parent*’s children"));
        assert_eq!(used(&index, &e), ["node"]);
    }

    #[test]
    fn algorithms_without_statements_still_get_steps() {
        let index = one(
            r##"<div class="algorithm"><p>To <dfn id="go">go</dfn>:</p><ol><li><p>Frobnicate <var>x</var>.</p></li><li><p>Return.</p></li></ol></div>"##,
        );
        assert_eq!(index.steps.len(), 2);
        assert!(index.edges.is_empty());
        assert_eq!(names(&index, "1"), ["x"]);
    }
}
