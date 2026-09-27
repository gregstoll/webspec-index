//! The statement tree across steps (§8.2): child blocks of control heads,
//! parents of statements in nested steps and bodies, `Otherwise` chains and
//! continuation statements.
use std::collections::HashMap;

use crate::parse::steps::{StepItem, StructuralAlgorithm, StructuralBody, StructuralStep};
use crate::state::ir::{
    statement_id, BlockRole, BodyLoc, SourceContext, Statement, StatementKind, StatementParent,
    StatementSource,
};
use crate::state::model::{ModelIssue, StateIssueCode};

const IN_PARALLEL: (&str, &str) = ("HTML", "in-parallel");

/// Where a source lives: its step, if any, and its body.
#[derive(Clone, Copy)]
struct Location<'a> {
    step: Option<&'a str>,
    body: &'a str,
}

struct Tree<'a> {
    steps: HashMap<&'a str, &'a StructuralStep>,
    bodies: HashMap<&'a str, &'a StructuralBody>,
    sources: HashMap<&'a str, (&'a StatementSource, Location<'a>)>,
    /// Statement indices per source id.
    by_source: HashMap<&'a str, Vec<usize>>,
    /// Step id → index of its head statement.
    heads: HashMap<&'a str, usize>,
}

/// Links the statements of one algorithm into a tree (§8.2): rewrites child
/// blocks to bodies where the steps live in one, gives each unparented
/// statement its enclosing control statement, resolves step-head
/// `Otherwise`s to their `If`, and adds a `ContinueRemaining` statement per
/// continuation site. Idempotent.
#[allow(dead_code)]
pub(crate) fn link_tree(
    algorithm: &StructuralAlgorithm,
    sources: &[StatementSource],
    statements: &mut Vec<Statement>,
    issues: &mut Vec<ModelIssue>,
) {
    let tree = Tree::new(algorithm, sources, statements);
    tree.rewrite_child_blocks(algorithm, statements);
    let (child_owners, body_owners) = tree.owners(statements);
    let enclosing = |location: Location<'_>, id: &str| {
        tree.enclosing(location, id, &child_owners, &body_owners)
    };
    for statement in statements.iter_mut() {
        if statement.parent.is_some() {
            continue;
        }
        let Some((_, location)) = tree.sources.get(statement.source_id.as_str()) else {
            continue;
        };
        statement.parent = enclosing(*location, &statement.id);
    }
    tree.resolve_otherwise(algorithm, statements, issues);

    for site in &algorithm.continuations {
        let Some(&(source, location)) = tree.sources.get(site.segment_id.as_str()) else {
            continue;
        };
        let id = statement_id(&source.id, "continue_remaining", site.span);
        let indices = tree.by_source.get(source.id.as_str());
        let in_source = || indices.into_iter().flatten().map(|&i| &statements[i]);
        if in_source().any(|s| s.id == id) {
            continue;
        }
        // A site outside every statement follows the last statement before
        // it: inside the inline block that statement opens, else beside it.
        let innermost = in_source()
            .filter(|s| s.span.start <= site.span.start && site.span.end <= s.span.end)
            .min_by_key(|s| s.span.end - s.span.start);
        let preceding = in_source()
            .filter(|s| s.span.start < site.span.start)
            .max_by_key(|s| s.span.start);
        let parent = match (innermost, preceding) {
            (Some(innermost), _) => innermost.parent.clone(),
            (None, Some(last)) => match block(&last.kind) {
                Some((BodyLoc::InlineRest, role)) => Some(parent(last, role)),
                _ => last.parent.clone(),
            },
            (None, None) => enclosing(location, &id),
        };
        statements.push(Statement {
            id,
            source_id: source.id.clone(),
            span: site.span,
            kind: StatementKind::ContinueRemaining {
                continuation_site: Some(site.source.node_id.clone()),
            },
            parent,
        });
    }
}

/// The block a control statement opens, with its role (adaptation 22).
fn block(kind: &StatementKind) -> Option<(&BodyLoc, BlockRole)> {
    match kind {
        StatementKind::If { then, .. } => Some((then, BlockRole::Then)),
        StatementKind::Otherwise { body, .. } => Some((body, BlockRole::Otherwise)),
        StatementKind::ForEach { body, .. }
        | StatementKind::While { body, .. }
        | StatementKind::InParallel { body }
        | StatementKind::RunSteps { body } => Some((body, BlockRole::Body)),
        _ => None,
    }
}

fn block_mut(kind: &mut StatementKind) -> Option<&mut BodyLoc> {
    match kind {
        StatementKind::If { then: body, .. }
        | StatementKind::Otherwise { body, .. }
        | StatementKind::ForEach { body, .. }
        | StatementKind::While { body, .. }
        | StatementKind::InParallel { body }
        | StatementKind::RunSteps { body } => Some(body),
        _ => None,
    }
}

/// Length of the step prefix (`⌛ `, `Optionally, `) the head may follow.
fn head_prefix_len(text: &str) -> usize {
    let rest = text.strip_prefix("\u{231B} ").unwrap_or(text);
    let rest = rest
        .strip_prefix("Optionally, ")
        .or_else(|| rest.strip_prefix("Optionally "))
        .unwrap_or(rest);
    text.len() - rest.len()
}

impl<'a> Tree<'a> {
    fn new(
        algorithm: &'a StructuralAlgorithm,
        sources: &'a [StatementSource],
        statements: &[Statement],
    ) -> Self {
        let steps: HashMap<&str, &StructuralStep> = algorithm
            .steps
            .iter()
            .map(|step| (step.source.node_id.as_str(), step))
            .collect();
        let bodies = algorithm
            .bodies
            .iter()
            .map(|body| (body.source.node_id.as_str(), body))
            .collect();
        let sources: HashMap<&str, (&StatementSource, Location)> = sources
            .iter()
            .filter_map(|source| {
                let location = match &source.context {
                    SourceContext::Algorithm {
                        step_id, body_id, ..
                    } => Location {
                        step: step_id.as_deref(),
                        body: body_id,
                    },
                    SourceContext::BranchLabel { step_id, .. } => Location {
                        step: Some(step_id),
                        body: &steps.get(step_id.as_str())?.body_id,
                    },
                    _ => return None,
                };
                Some((source.id.as_str(), (source, location)))
            })
            .collect();
        let mut by_source: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, statement) in statements.iter().enumerate() {
            if let Some((key, _)) = sources.get_key_value(statement.source_id.as_str()) {
                by_source.entry(key).or_default().push(index);
            }
        }
        let heads = algorithm
            .steps
            .iter()
            .filter_map(|step| {
                let first = step.items.iter().find_map(|item| match item {
                    StepItem::Segment(id) => Some(id.as_str()),
                    _ => None,
                })?;
                let (source, _) = sources.get(first)?;
                let head = by_source
                    .get(first)?
                    .iter()
                    .copied()
                    .min_by_key(|&i| statements[i].span.start)?;
                (statements[head].span.start <= head_prefix_len(&source.text))
                    .then_some((step.source.node_id.as_str(), head))
            })
            .collect();
        Tree {
            steps,
            bodies,
            sources,
            by_source,
            heads,
        }
    }

    /// `ChildSteps { S }` of a step whose children form a body becomes that
    /// body; an `InParallel` whose head link starts a body gets that body.
    fn rewrite_child_blocks(&self, algorithm: &StructuralAlgorithm, statements: &mut [Statement]) {
        for indices in self.by_source.values() {
            for &index in indices {
                let statement = &mut statements[index];
                let in_parallel = match statement.kind {
                    StatementKind::InParallel { .. } => self.in_parallel_body(algorithm, statement),
                    _ => None,
                };
                let Some(body) = block_mut(&mut statement.kind) else {
                    continue;
                };
                if let Some(body_id) = in_parallel {
                    *body = BodyLoc::Body { body_id };
                } else if let BodyLoc::ChildSteps { step_id } = body {
                    if let Some(body_id) = self.steps.get(step_id.as_str()).and_then(|step| {
                        let has_children = step
                            .items
                            .iter()
                            .any(|item| matches!(item, StepItem::ChildStep(_)));
                        step.items.iter().find_map(|item| match item {
                            StepItem::Body(b) if !has_children => Some(b.clone()),
                            _ => None,
                        })
                    }) {
                        *body = BodyLoc::Body { body_id };
                    }
                }
            }
        }
    }

    /// The first body started by a link to `HTML#in-parallel` inside the
    /// statement's span.
    fn in_parallel_body(
        &self,
        algorithm: &StructuralAlgorithm,
        statement: &Statement,
    ) -> Option<String> {
        let (source, _) = self.sources.get(statement.source_id.as_str())?;
        source
            .links
            .iter()
            .filter(|link| {
                link.span.start >= statement.span.start
                    && link.span.end <= statement.span.end
                    && link.target.as_ref().is_some_and(|t| {
                        t.spec.eq_ignore_ascii_case(IN_PARALLEL.0) && t.anchor == IN_PARALLEL.1
                    })
            })
            .find_map(|link| {
                algorithm
                    .operation_sites
                    .iter()
                    .find(|site| site.segment_id == source.id && site.link_id == link.id)
                    .and_then(|site| site.actual_body_ids.first().cloned())
            })
    }

    /// Step id → the head statement opening its child steps; body id → the
    /// statement opening that body.
    #[allow(clippy::type_complexity)]
    fn owners(
        &self,
        statements: &[Statement],
    ) -> (
        HashMap<&'a str, StatementParent>,
        HashMap<String, StatementParent>,
    ) {
        let mut child_owners = HashMap::new();
        for (&step, &head) in &self.heads {
            if let Some((BodyLoc::ChildSteps { step_id }, role)) = block(&statements[head].kind) {
                if step_id == step {
                    child_owners.insert(step, parent(&statements[head], role));
                }
            }
        }
        let mut body_owners = HashMap::new();
        let mut indices: Vec<usize> = self.by_source.values().flatten().copied().collect();
        indices.sort_unstable();
        for index in indices {
            if let Some((BodyLoc::Body { body_id }, role)) = block(&statements[index].kind) {
                body_owners
                    .entry(body_id.clone())
                    .or_insert_with(|| parent(&statements[index], role));
            }
        }
        (child_owners, body_owners)
    }

    /// The nearest enclosing control statement of a statement `id` at
    /// `location`: through proper ancestor steps whose head opens their child
    /// steps, and through bodies up to the step defining them.
    fn enclosing(
        &self,
        location: Location<'_>,
        id: &str,
        child_owners: &HashMap<&str, StatementParent>,
        body_owners: &HashMap<String, StatementParent>,
    ) -> Option<StatementParent> {
        let other = |owner: Option<&StatementParent>| {
            owner.filter(|owner| owner.statement_id != id).cloned()
        };
        let by_body = |body: &str| -> (Option<StatementParent>, Option<&'a str>) {
            (
                other(body_owners.get(body)),
                self.bodies
                    .get(body)
                    .and_then(|b| b.defined_at_step_id.as_deref()),
            )
        };
        let mut step = match location.step.and_then(|s| self.steps.get(s)) {
            Some(step) if step.body_id == location.body => *step,
            _ => {
                let (owner, defined_at) = by_body(location.body);
                if owner.is_some() {
                    return owner;
                }
                *location
                    .step
                    .or(defined_at)
                    .and_then(|s| self.steps.get(s))?
            }
        };
        for _ in 0..=self.steps.len() + self.bodies.len() {
            if let Some(parent_id) = step.parent_step_id.as_deref() {
                if let Some(owner) = other(child_owners.get(parent_id)) {
                    return Some(owner);
                }
                step = self.steps.get(parent_id)?;
                continue;
            }
            let (owner, defined_at) = by_body(&step.body_id);
            if owner.is_some() {
                return owner;
            }
            step = self.steps.get(defined_at?)?;
        }
        None
    }

    /// Step-head `Otherwise`s without `of` get the nearest preceding sibling
    /// step's `If` or conditional `Otherwise`; none is a `DanglingOtherwise`.
    fn resolve_otherwise(
        &self,
        algorithm: &StructuralAlgorithm,
        statements: &mut [Statement],
        issues: &mut Vec<ModelIssue>,
    ) {
        let mut heads: Vec<(&str, usize)> = self.heads.iter().map(|(&s, &i)| (s, i)).collect();
        heads.sort_unstable_by_key(|&(_, index)| index);
        for (step_id, index) in heads {
            if !matches!(
                statements[index].kind,
                StatementKind::Otherwise { of: None, .. }
            ) {
                continue;
            }
            let step = self.steps[step_id];
            let chained = algorithm
                .steps
                .iter()
                .filter(|s| {
                    s.parent_step_id == step.parent_step_id
                        && s.body_id == step.body_id
                        && s.ordinal < step.ordinal
                })
                .filter_map(|s| {
                    let head = &statements[*self.heads.get(s.source.node_id.as_str())?];
                    matches!(
                        head.kind,
                        StatementKind::If { .. }
                            | StatementKind::Otherwise {
                                condition: Some(_),
                                ..
                            }
                    )
                    .then(|| (s.ordinal, head.id.clone()))
                })
                .max_by_key(|&(ordinal, _)| ordinal)
                .map(|(_, id)| id);
            match chained {
                Some(if_id) => {
                    if let StatementKind::Otherwise { of, .. } = &mut statements[index].kind {
                        *of = Some(if_id);
                    }
                }
                None => {
                    let path = step
                        .path
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(".");
                    issues.push(ModelIssue {
                        code: StateIssueCode::DanglingOtherwise,
                        anchor: Some(algorithm.source.section_anchor.clone()),
                        message: format!("{path}: Otherwise without a preceding If"),
                    });
                }
            }
        }
    }
}

fn parent(statement: &Statement, role: BlockRole) -> StatementParent {
    StatementParent {
        statement_id: statement.id.clone(),
        role,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::steps::extract_step_structure;
    use crate::state::grammar::Env;
    use crate::state::ir::{parse_source_with, BlockRole, StatementKind};
    use crate::state::testing::{FALLBACK_HTML, NAV_HTML};

    fn linked(html: &str, anchor: &str) -> (Vec<Statement>, Vec<ModelIssue>) {
        let structure =
            extract_step_structure(html, "HTML", "https://html.spec.whatwg.org/", "hash:t");
        let algorithm = structure
            .algorithms
            .iter()
            .find(|a| a.source.section_anchor == anchor)
            .unwrap();
        let sources: Vec<_> = crate::state::extract::algorithm_sources(&structure)
            .0
            .into_iter()
            .filter(|s| s.subject.anchor == anchor)
            .collect();
        let mut statements: Vec<Statement> = sources
            .iter()
            .flat_map(|s| parse_source_with(s, &Env::default()).statements)
            .collect();
        let mut issues = Vec::new();
        link_tree(algorithm, &sources, &mut statements, &mut issues);
        (statements, issues)
    }

    #[test]
    fn child_steps_of_an_if_point_to_it() {
        let (st, _) = linked(FALLBACK_HTML, "fallback-base-url");
        let if_stmt = st
            .iter()
            .find(|s| matches!(s.kind, StatementKind::If { .. }))
            .unwrap();
        let assert = st
            .iter()
            .find(|s| matches!(s.kind, StatementKind::Assert { .. }))
            .unwrap();
        let ret = st
            .iter()
            .filter(|s| matches!(s.kind, StatementKind::Return { .. }))
            .collect::<Vec<_>>();
        assert_eq!(
            assert
                .parent
                .as_ref()
                .map(|p| (p.statement_id.as_str(), p.role)),
            Some((if_stmt.id.as_str(), BlockRole::Then))
        );
        assert_eq!(ret[0].parent.as_ref().unwrap().statement_id, if_stmt.id);
        assert_eq!(ret[1].parent, None, "step 2 is not inside the If");
        assert_eq!(if_stmt.parent, None, "the If is not its own parent");
    }

    #[test]
    fn in_parallel_block() {
        let (st, _) = linked(NAV_HTML, "navigate");
        let par = st
            .iter()
            .find(|s| matches!(s.kind, StatementKind::InParallel { .. }))
            .unwrap();
        let set = st
            .iter()
            .find(|s| matches!(s.kind, StatementKind::Set { .. }))
            .unwrap();
        assert_eq!(
            set.parent
                .as_ref()
                .map(|p| (p.statement_id.as_str(), p.role)),
            Some((par.id.as_str(), BlockRole::Body))
        );
    }

    #[test]
    fn steps_in_a_body_point_to_its_owner() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="d">d</dfn> given <var>x</var>:</p><ol><li><p>Run the following steps <a href="#in-parallel">in parallel</a>:</p><ol><li><p>If <var>x</var> is null, then return.</p><li><p>Set <var>x</var>'s <a href="#f">f</a> to null.</p></li></ol></li></ol></div>"##;
        let structure =
            extract_step_structure(html, "HTML", "https://html.spec.whatwg.org/", "hash:t");
        let body_id = structure.algorithms[0].steps[0]
            .items
            .iter()
            .find_map(|item| match item {
                StepItem::Body(b) => Some(b.clone()),
                _ => None,
            })
            .expect("the in-parallel steps form a body");
        let (st, _) = linked(html, "d");
        let par = st
            .iter()
            .find(|s| matches!(s.kind, StatementKind::InParallel { .. }))
            .unwrap();
        assert_eq!(
            par.kind,
            StatementKind::InParallel {
                body: BodyLoc::Body { body_id }
            }
        );
        assert_eq!(par.parent, None);
        let parent_of = |pick: fn(&StatementKind) -> bool| {
            st.iter()
                .find(|s| pick(&s.kind))
                .and_then(|s| s.parent.clone())
                .map(|p| (p.statement_id, p.role))
        };
        let if_id = st
            .iter()
            .find(|s| matches!(s.kind, StatementKind::If { .. }))
            .unwrap()
            .id
            .clone();
        assert_eq!(
            parent_of(|k| matches!(k, StatementKind::If { .. })),
            Some((par.id.clone(), BlockRole::Body))
        );
        assert_eq!(
            parent_of(|k| matches!(k, StatementKind::Set { .. })),
            Some((par.id.clone(), BlockRole::Body))
        );
        assert_eq!(
            parent_of(|k| matches!(k, StatementKind::Return { .. })),
            Some((if_id, BlockRole::Then)),
            "the inline block keeps its If"
        );
    }

    #[test]
    fn otherwise_steps_resolve_or_dangle() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="a">a</dfn> given <var>x</var>:</p><ol><li><p>If <var>x</var> is null, then return.</p></li><li><p>Otherwise, return <var>x</var>.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="b">b</dfn> given <var>x</var>:</p><ol><li><p>Let <var>y</var> be 1.</p></li><li><p>Otherwise, return.</p></li></ol></div>"##;
        let (st, issues) = linked(html, "a");
        let if_id = st
            .iter()
            .find(|s| matches!(s.kind, StatementKind::If { .. }))
            .unwrap()
            .id
            .clone();
        assert!(st.iter().any(
            |s| matches!(&s.kind, StatementKind::Otherwise { of: Some(of), .. } if of == &if_id)
        ));
        assert!(issues.is_empty());
        let (_, issues) = linked(html, "b");
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].code,
            crate::state::model::StateIssueCode::DanglingOtherwise
        );
        assert_eq!(issues[0].anchor.as_deref(), Some("b"));
        assert_eq!(issues[0].message, "2: Otherwise without a preceding If");
    }

    #[test]
    fn continuation_sites_become_continue_remaining() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="c">c</dfn> given <var>done</var>:</p><ol><li><p>Wait until <var>done</var> is true, and then continue with the remaining steps.</p></li><li><p>Return.</p></li></ol></div>"##;
        let (st, _) = linked(html, "c");
        assert!(st.iter().any(|s| matches!(
            &s.kind,
            StatementKind::ContinueRemaining {
                continuation_site: Some(_)
            }
        )));
        assert!(st.iter().any(|s| matches!(
            &s.kind,
            StatementKind::Wait {
                condition: Some(_),
                ..
            }
        )));
    }

    #[test]
    fn continuation_in_an_inline_block_takes_its_head() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="c">c</dfn> given <var>p</var>:</p><ol><li><p>If <var>p</var> is true, then continue these steps.</p></li><li><p>Return.</p></li></ol></div>"##;
        let (st, _) = linked(html, "c");
        let if_id = &st
            .iter()
            .find(|s| matches!(s.kind, StatementKind::If { .. }))
            .unwrap()
            .id;
        let site = st
            .iter()
            .find(|s| matches!(s.kind, StatementKind::ContinueRemaining { .. }))
            .expect("the site becomes a statement");
        assert_eq!(
            site.parent
                .as_ref()
                .map(|p| (p.statement_id.as_str(), p.role)),
            Some((if_id.as_str(), BlockRole::Then))
        );
    }

    #[test]
    fn linking_twice_changes_nothing() {
        for (html, anchor) in [(FALLBACK_HTML, "fallback-base-url"), (NAV_HTML, "navigate")] {
            let structure =
                extract_step_structure(html, "HTML", "https://html.spec.whatwg.org/", "hash:t");
            let algorithm = structure
                .algorithms
                .iter()
                .find(|a| a.source.section_anchor == anchor)
                .unwrap();
            let (st, _) = linked(html, anchor);
            let sources: Vec<_> = crate::state::extract::algorithm_sources(&structure)
                .0
                .into_iter()
                .filter(|s| s.subject.anchor == anchor)
                .collect();
            let mut again = st.clone();
            let mut issues = Vec::new();
            link_tree(algorithm, &sources, &mut again, &mut issues);
            assert_eq!(again, st, "{anchor}");
        }
    }
}
