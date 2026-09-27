//! Variable definitions, uses and origins (§7.6), over one [`IrIndex`] per
//! `StateSpec`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;

use crate::parse::steps::{AnchorTarget, InlineTokenKind};
use crate::state::ir::{
    Call, Expr, Origin, Path, Predicate, Root, RunContext, SourceContext, Statement, StatementKind,
    StatementSource, VarOrigin, VarOrigins,
};
use crate::state::model::{Signature, StateSpec};

/// Built once per StateSpec; every per-statement or per-subject lookup goes through it.
pub struct IrIndex<'a> {
    /// call id → call
    pub calls: HashMap<&'a str, &'a Call>,
    /// source id → source
    pub sources: HashMap<&'a str, &'a StatementSource>,
    /// document order
    pub sources_by_subject: BTreeMap<&'a AnchorTarget, Vec<&'a StatementSource>>,
    /// source then span order
    pub statements_by_subject: BTreeMap<&'a AnchorTarget, Vec<&'a Statement>>,
    /// span order
    pub calls_by_source: HashMap<&'a str, Vec<&'a Call>>,
}

impl<'a> IrIndex<'a> {
    pub fn new(state: &'a StateSpec) -> Self {
        let mut sources = HashMap::with_capacity(state.sources.len());
        let mut position = HashMap::with_capacity(state.sources.len());
        let mut sources_by_subject: BTreeMap<&AnchorTarget, Vec<&StatementSource>> =
            BTreeMap::new();
        for (i, source) in state.sources.iter().enumerate() {
            sources.insert(source.id.as_str(), source);
            position.insert(source.id.as_str(), i);
            sources_by_subject
                .entry(&source.subject)
                .or_default()
                .push(source);
        }

        let mut keyed: BTreeMap<&AnchorTarget, Vec<(usize, usize, &Statement)>> = BTreeMap::new();
        for statement in &state.statements {
            let Some(source) = sources.get(statement.source_id.as_str()) else {
                continue;
            };
            keyed.entry(&source.subject).or_default().push((
                position[statement.source_id.as_str()],
                statement.span.start,
                statement,
            ));
        }
        let statements_by_subject = keyed
            .into_iter()
            .map(|(subject, mut list)| {
                list.sort_by_key(|&(pos, start, _)| (pos, start));
                (subject, list.into_iter().map(|(.., s)| s).collect())
            })
            .collect();

        let mut calls = HashMap::with_capacity(state.calls.len());
        let mut calls_by_source: HashMap<&str, Vec<&Call>> = HashMap::new();
        for call in &state.calls {
            calls.insert(call.id.as_str(), call);
            calls_by_source
                .entry(call.source_id.as_str())
                .or_default()
                .push(call);
        }
        for list in calls_by_source.values_mut() {
            list.sort_by_key(|call| call.span.start);
        }

        IrIndex {
            calls,
            sources,
            sources_by_subject,
            statements_by_subject,
            calls_by_source,
        }
    }
}

fn bare_var(path: &Path) -> Option<&str> {
    match &path.root {
        Root::Var(name) if path.hops.is_empty() && path.subscript.is_none() => Some(name),
        _ => None,
    }
}

/// The variables `statement` defines: a `Let`'s variable, a `ForEach`'s loop
/// variables, and the bare-variable targets of a `Set`.
pub fn defs(statement: &Statement) -> Vec<&str> {
    match &statement.kind {
        StatementKind::Let { var, .. } => vec![var.as_str()],
        StatementKind::ForEach { vars, .. } => vars.iter().map(String::as_str).collect(),
        StatementKind::Set { targets, .. } => targets.iter().filter_map(bare_var).collect(),
        _ => Vec::new(),
    }
}

/// The variables `statement` reads, without duplicates, in first-occurrence order.
pub fn uses<'a>(statement: &'a Statement, index: &IrIndex<'a>) -> Vec<&'a str> {
    let mut uses = Uses {
        index,
        seen: HashSet::new(),
        out: Vec::new(),
    };
    match &statement.kind {
        StatementKind::Let { value, .. } => uses.expr(value),
        StatementKind::Set { targets, value, .. } => {
            for target in targets {
                if bare_var(target).is_none() {
                    uses.path(target);
                }
            }
            uses.expr(value);
        }
        StatementKind::Mutate {
            target, operand, ..
        } => {
            if let Some(operand) = operand {
                uses.expr(operand);
            }
            uses.path(target);
        }
        StatementKind::Init { entries, .. } => {
            for entry in entries {
                uses.expr(&entry.value);
            }
        }
        StatementKind::Call { call } => uses.call(call),
        StatementKind::If { condition, .. } => uses.predicate(condition),
        StatementKind::Otherwise { condition, .. }
        | StatementKind::While { condition, .. }
        | StatementKind::Wait { condition, .. } => {
            if let Some(condition) = condition {
                uses.predicate(condition);
            }
        }
        StatementKind::ForEach {
            collection, filter, ..
        } => {
            uses.expr(collection);
            if let Some(filter) = filter {
                uses.predicate(filter);
            }
        }
        StatementKind::Return { value } => {
            if let Some(value) = value {
                uses.expr(value);
            }
        }
        StatementKind::Assert { predicate } => uses.predicate(predicate),
        StatementKind::Opaque { .. }
        | StatementKind::Throw { .. }
        | StatementKind::Abort { .. }
        | StatementKind::Continue
        | StatementKind::Break
        | StatementKind::ContinueRemaining { .. }
        | StatementKind::InParallel { .. }
        | StatementKind::RunSteps { .. } => {}
    }
    uses.out
}

struct Uses<'a, 'i> {
    index: &'i IrIndex<'a>,
    seen: HashSet<&'a str>,
    out: Vec<&'a str>,
}

impl<'a> Uses<'a, '_> {
    fn push(&mut self, name: &'a str) {
        if self.seen.insert(name) {
            self.out.push(name);
        }
    }

    fn expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Var(name) => self.push(name),
            Expr::Path(path) => self.path(path),
            Expr::Call(id) => self.call(id),
            Expr::List(items) => items.iter().for_each(|item| self.expr(item)),
            Expr::Conditional {
                condition,
                then,
                otherwise,
            } => {
                self.predicate(condition);
                self.expr(then);
                self.expr(otherwise);
            }
            Expr::This
            | Expr::Literal(_)
            | Expr::New { .. }
            | Expr::Opaque { .. }
            | Expr::AlgorithmRef { .. }
            | Expr::EnumValue { .. } => {}
        }
    }

    fn path(&mut self, path: &'a Path) {
        if let Root::Var(name) = &path.root {
            self.push(name);
        }
        if let Some(subscript) = &path.subscript {
            self.expr(subscript);
        }
    }

    /// The `Variable` tokens inside the call's region, then its receiver.
    fn call(&mut self, id: &str) {
        let index = self.index;
        let Some(call) = index.calls.get(id).copied() else {
            return;
        };
        if let Some(source) = index.sources.get(call.source_id.as_str()).copied() {
            for token in &source.tokens {
                if token.kind == InlineTokenKind::Variable
                    && token.span.start >= call.region.start
                    && token.span.end <= call.region.end
                {
                    self.push(&token.source_text);
                }
            }
        }
        if let Some(receiver) = &call.receiver {
            self.expr(receiver);
        }
    }

    fn predicate(&mut self, predicate: &'a Predicate) {
        match predicate {
            Predicate::Is { operand, .. } | Predicate::Exists { operand, .. } => self.expr(operand),
            Predicate::Compare { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Predicate::OneOf {
                operand, values, ..
            } => {
                self.expr(operand);
                values.iter().for_each(|value| self.expr(value));
            }
            Predicate::Contains {
                container, item, ..
            } => {
                self.expr(container);
                self.expr(item);
            }
            Predicate::HasAttribute { element, .. } => self.expr(element),
            Predicate::Holds { subjects, call, .. } => {
                subjects.iter().for_each(|subject| self.expr(subject));
                if let Some(call) = call {
                    self.call(call);
                }
            }
            Predicate::RunningOn { context } => match context {
                RunContext::InParallel => {}
                RunContext::Queue(expr) | RunContext::EventLoopTask(expr) => self.expr(expr),
            },
            Predicate::And(items) | Predicate::Or(items) => {
                items.iter().for_each(|item| self.predicate(item))
            }
            Predicate::Implies(lhs, rhs) => {
                self.predicate(lhs);
                self.predicate(rhs);
            }
            Predicate::Opaque { .. } => {}
        }
    }
}

/// Text before a body parameter: `steps given `, `steps, given ` or
/// `algorithm given `, optionally followed by a type phrase.
fn body_param_lead() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:steps,? |algorithm )given (?:[^\s,.;:()]+ ){0,4}$").unwrap())
}

struct Origins<'a> {
    vars: Vec<VarOrigin>,
    at: HashMap<&'a str, usize>,
}

impl<'a> Origins<'a> {
    fn mention(&mut self, name: &'a str) -> usize {
        *self.at.entry(name).or_insert_with(|| {
            self.vars.push(VarOrigin {
                name: name.to_string(),
                origin: Origin::Undeclared,
            });
            self.vars.len() - 1
        })
    }

    /// Mentions `name` and gives it `origin` unless it already has one.
    fn define(
        &mut self,
        name: &'a str,
        origin: impl FnOnce() -> Origin,
        defined: &mut HashSet<usize>,
    ) {
        let i = self.mention(name);
        if defined.insert(i) {
            self.vars[i].origin = origin();
        }
    }
}

/// Where each variable of `subject` comes from; names are listed in
/// first-mention order, parameters first.
#[allow(dead_code)]
pub(crate) fn var_origins(
    subject: &AnchorTarget,
    signature: Option<&Signature>,
    index: &IrIndex,
) -> VarOrigins {
    let mut origins = Origins {
        vars: Vec::new(),
        at: HashMap::new(),
    };
    let mut defined = HashSet::new();
    for (i, param) in signature.iter().flat_map(|s| &s.params).enumerate() {
        origins.define(
            &param.name,
            || Origin::Param { index: i as u32 },
            &mut defined,
        );
    }

    let statements = index
        .statements_by_subject
        .get(subject)
        .map_or(&[][..], Vec::as_slice);
    let sources = index
        .sources_by_subject
        .get(subject)
        .map_or(&[][..], Vec::as_slice);
    let mut next = 0;
    for source in sources {
        if matches!(source.context, SourceContext::Intro { .. }) {
            continue;
        }
        let end = statements[next..]
            .iter()
            .position(|s| s.source_id != source.id)
            .map_or(statements.len(), |i| next + i);
        let own = &statements[next..end];
        next = end;

        let body_params = body_params(source, index);
        let mut params = body_params.iter().peekable();
        for statement in own {
            while let Some((name, body_id, _)) =
                params.next_if(|(_, _, at)| *at < statement.span.start)
            {
                let body_id = body_id.clone();
                origins.define(name, || Origin::BodyParam { body_id }, &mut defined);
            }
            let id = statement.id.as_str();
            match &statement.kind {
                StatementKind::Let { var, .. } => origins.define(
                    var,
                    || Origin::Let {
                        statement_id: id.to_string(),
                    },
                    &mut defined,
                ),
                StatementKind::ForEach { vars, .. } => {
                    for var in vars {
                        origins.define(
                            var,
                            || Origin::LoopVar {
                                statement_id: id.to_string(),
                            },
                            &mut defined,
                        );
                    }
                }
                StatementKind::Set { targets, .. } => {
                    for var in targets.iter().filter_map(bare_var) {
                        origins.define(
                            var,
                            || Origin::Let {
                                statement_id: id.to_string(),
                            },
                            &mut defined,
                        );
                    }
                }
                _ => {}
            }
            for name in uses(statement, index) {
                origins.mention(name);
            }
        }
        for (name, body_id, _) in params {
            let body_id = body_id.clone();
            origins.define(name, || Origin::BodyParam { body_id }, &mut defined);
        }
    }
    VarOrigins {
        subject: subject.clone(),
        vars: origins.vars,
    }
}

/// `(name, body_id, offset)` of each body parameter token of `source`.
fn body_params<'a>(source: &'a StatementSource, index: &IrIndex) -> Vec<(&'a str, String, usize)> {
    let variables: Vec<_> = source
        .tokens
        .iter()
        .filter(|token| token.kind == InlineTokenKind::Variable)
        .collect();
    let mut found = Vec::new();
    for token in &variables {
        let Some(lead) = body_param_lead().find(&source.text[..token.span.start]) else {
            continue;
        };
        let given_end = lead.start() + lead.as_str().find("given ").unwrap_or(0) + "given ".len();
        let between = variables
            .iter()
            .any(|other| other.span.start >= given_end && other.span.start < token.span.start);
        if between {
            continue;
        }
        let body_id = index
            .calls_by_source
            .get(source.id.as_str())
            .and_then(|calls| calls.iter().find_map(|call| call.body_args.first()))
            .cloned()
            .unwrap_or_default();
        found.push((token.source_text.as_str(), body_id, token.span.start));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::grammar::Env;
    use crate::state::ir::{parse_source_with, Origin};
    use crate::state::testing::{extract_html, INSERT_DOM};

    fn origins(html: &str, spec: &str, anchor: &str) -> Vec<(String, Origin)> {
        let mut state = extract_html(html, spec);
        let subject = state
            .sources
            .iter()
            .find(|s| s.subject.anchor == anchor)
            .unwrap()
            .subject
            .clone();
        state.statements = state
            .sources
            .iter()
            .filter(|s| !s.id.starts_with("intro-"))
            .flat_map(|s| parse_source_with(s, &Env::default()).statements)
            .collect();
        let signature = state
            .signatures
            .iter()
            .find(|s| s.algorithm == subject)
            .cloned();
        let index = IrIndex::new(&state);
        var_origins(&subject, signature.as_ref(), &index)
            .vars
            .into_iter()
            .map(|v| (v.name, v.origin))
            .collect()
    }

    #[test]
    fn call_regions_give_uses_and_body_params_their_origin() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="v">v</dfn> given a string <var>p</var> and a string <var>q</var>:</p><ol><li><p>Return.</p></li></ol></div><div data-algorithm=""><p>To <dfn id="u">u</dfn>:</p><ol><li><p>Let <var>r</var> be the result of running <a href="#v">v</a> given <var>a</var> and <var>b</var>.</p></li><li><p>Run the following steps given a response <var>resp</var>:</p><ol><li><p>Set <var>r</var> to <var>resp</var>.</p></li></ol></li></ol></div>"##;
        let env = Env {
            spec: "HTML".into(),
            callables: [("v".to_string(), crate::state::grammar::Callable::Template)].into(),
            ..Env::default()
        };
        let mut state = extract_html(html, "HTML");
        let (mut statements, mut calls) = (Vec::new(), Vec::new());
        for source in state.sources.iter().filter(|s| !s.id.starts_with("intro-")) {
            let parsed = parse_source_with(source, &env);
            statements.extend(parsed.statements);
            calls.extend(parsed.calls);
        }
        (state.statements, state.calls) = (statements, calls);
        let index = IrIndex::new(&state);
        let let_r = state
            .statements
            .iter()
            .find(|s| matches!(&s.kind, StatementKind::Let { var, .. } if var == "r"))
            .unwrap();
        assert_eq!(defs(let_r), vec!["r"]);
        assert_eq!(uses(let_r, &index), vec!["a", "b"]);

        let subject = index
            .sources_by_subject
            .keys()
            .find(|s| s.anchor == "u")
            .copied()
            .unwrap();
        let o: Vec<(String, Origin)> = var_origins(subject, None, &index)
            .vars
            .into_iter()
            .map(|v| (v.name, v.origin))
            .collect();
        assert!(
            matches!(&o[0], (n, Origin::Let { statement_id }) if n == "r" && statement_id == &let_r.id)
        );
        assert_eq!(
            o[1..3],
            [
                ("a".into(), Origin::Undeclared),
                ("b".into(), Origin::Undeclared)
            ]
        );
        assert!(o.contains(&(
            "resp".into(),
            Origin::BodyParam {
                body_id: String::new()
            }
        )));
        assert_eq!(o.iter().filter(|(n, _)| n == "r").count(), 1);
    }

    #[test]
    fn index_groups_by_id_subject_and_source() {
        let state = extract_html(INSERT_DOM, "DOM");
        let index = IrIndex::new(&state);
        assert_eq!(index.sources.len(), state.sources.len());
        let subject = &state.sources[0].subject;
        let listed: usize = index.statements_by_subject.values().map(Vec::len).sum();
        assert_eq!(listed, state.statements.len());
        assert!(index.sources_by_subject[subject]
            .iter()
            .all(|s| &s.subject == subject));
    }

    #[test]
    fn params_lets_loop_variables_and_undeclared() {
        let o = origins(INSERT_DOM, "DOM", "concept-node-insert");
        assert_eq!(o[0], ("node".into(), Origin::Param { index: 0 }));
        assert!(o
            .iter()
            .any(|(n, g)| n == "nodes" && matches!(g, Origin::Let { .. })));
        assert!(
            o.iter().filter(|(n, _)| n == "node").count() == 1,
            "a loop over a parameter keeps the parameter origin"
        );
        let html = r##"<div data-algorithm=""><p>To <dfn id="u">u</dfn> given a string <var>a</var>:</p><ol><li><p>Set <var>b</var>'s <a href="#f">f</a> to <var>a</var>.</p></li><li><p>Set <var>c</var> to <var>a</var>.</p></li></ol></div>"##;
        let o = origins(html, "HTML", "u");
        assert!(o.contains(&("a".into(), Origin::Param { index: 0 })));
        assert!(o.contains(&("b".into(), Origin::Undeclared)));
        assert!(o
            .iter()
            .any(|(n, g)| n == "c" && matches!(g, Origin::Let { .. })));
    }
}
