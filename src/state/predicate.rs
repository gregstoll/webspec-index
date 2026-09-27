//! `PRED` grammar (§8.3): one predicate grammar for conditions and
//! assertions.
use crate::state::grammar::{Callability, Callable, Parser, Placeholder};
use crate::state::ir::{CallForm, CmpOp, Expr, LinkRole, Predicate, RunContext, Test};
use crate::state::model::{Literal, TypeExpr, TypeKey, TypeRef};
use crate::state::typeexpr::{infra_word_kind, primitive_phrase};

const POSS: [&str; 2] = ["'s ", "\u{2019}s "];
/// Words a nominal `TYPE` may be followed by.
const TYPE_WORDS: [&str; 3] = [" object", " element", " node"];
/// Comparison phrases after `is (not )?`, longest first.
const COMPARISONS: [(&str, CmpOp); 6] = [
    ("greater than or equal to ", CmpOp::Ge),
    ("less than or equal to ", CmpOp::Le),
    ("greater than ", CmpOp::Gt),
    ("less than ", CmpOp::Lt),
    ("at least ", CmpOp::Ge),
    ("at most ", CmpOp::Le),
];

#[derive(Clone, Copy)]
enum Verb {
    Is,
    Equals,
    Contains,
    Has,
    Exists,
}

/// The verbs an atom splits at, with whether they negate.
const VERBS: [(&str, Verb, bool); 10] = [
    (" is ", Verb::Is, false),
    (" are ", Verb::Is, false),
    (" equals ", Verb::Equals, false),
    (" does not equal ", Verb::Equals, true),
    (" contains ", Verb::Contains, false),
    (" does not contain ", Verb::Contains, true),
    (" has ", Verb::Has, false),
    (" does not have ", Verb::Has, true),
    (" exists", Verb::Exists, false),
    (" does not exist", Verb::Exists, true),
];

impl Parser<'_> {
    /// `PRED` → `Predicate`: an atom, else an implication, a disjunction or
    /// a conjunction of atoms, else wholly `Opaque`. Only a parsed predicate
    /// leaves link roles and calls behind.
    pub(crate) fn predicate_at(&mut self, start: usize, end: usize) -> Predicate {
        let (start, end) = self.trimmed(start, end);
        if let Some(pred) = self.attempt(|p| p.atom(start, end)) {
            return pred;
        }
        if let Some(pred) = self.attempt(|p| p.implication(start, end)) {
            return pred;
        }
        if let Some(pred) = self.attempt(|p| p.disjunction(start, end)) {
            return pred;
        }
        Predicate::Opaque {
            text: self.src_text(start, end),
        }
    }

    /// Runs `f`, undoing the roles and calls it recorded when it fails.
    fn attempt<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        let roles = self.out.roles.clone();
        let calls = self.out.calls.len();
        let result = f(self);
        if result.is_none() {
            self.out.roles = roles;
            self.out.calls.truncate(calls);
        }
        result
    }

    /// `if ` CONJ `, then ` CONJ.
    fn implication(&mut self, start: usize, end: usize) -> Option<Predicate> {
        let condition = self.keyword(start, &["if "])?;
        let then_at = self
            .top_level(condition, end)
            .into_iter()
            .find(|&pos| self.lit(pos, ", then "))?;
        let condition = self.conjunction(condition, then_at)?;
        let consequence = self.conjunction(then_at + ", then ".len(), end)?;
        Some(Predicate::Implies(
            Box::new(condition),
            Box::new(consequence),
        ))
    }

    /// CONJ parts between top-level `, or `/` or `.
    fn disjunction(&mut self, start: usize, end: usize) -> Option<Predicate> {
        let parts = self.split_top(start, end, &[", or ", " or "]);
        if parts.len() == 1 {
            return self.conjunction(start, end);
        }
        parts
            .into_iter()
            .map(|(start, end)| self.conjunction(start, end))
            .collect::<Option<Vec<_>>>()
            .map(Predicate::Or)
    }

    /// Atoms between top-level `, and `/` and `.
    fn conjunction(&mut self, start: usize, end: usize) -> Option<Predicate> {
        let parts = self.split_top(start, end, &[", and ", " and "]);
        if parts.len() == 1 {
            return self.atom(start, end);
        }
        parts
            .into_iter()
            .map(|(start, end)| self.atom(start, end))
            .collect::<Option<Vec<_>>>()
            .map(Predicate::And)
    }

    /// The ranges between top-level occurrences of `separators` (longest
    /// first where they share a start).
    fn split_top(&self, start: usize, end: usize, separators: &[&str]) -> Vec<(usize, usize)> {
        let mut parts = Vec::new();
        let mut part_start = start;
        for pos in self.top_level(start, end) {
            if pos < part_start || self.inside_comparison(pos) {
                continue;
            }
            if let Some(separator) = separators
                .iter()
                .find(|separator| pos + separator.len() <= end && self.lit(pos, separator))
            {
                parts.push((part_start, pos));
                part_start = pos + separator.len();
            }
        }
        parts.push((part_start, end));
        parts
    }

    /// Whether `pos` is the ` or ` of `greater/less than or equal to`.
    fn inside_comparison(&self, pos: usize) -> bool {
        self.enc.text[..pos].ends_with(" than") && self.lit(pos, " or equal to ")
    }

    /// `OPERAND`: an expression that is a variable, `this`, a literal, a
    /// path, a call, an enum value or a list.
    fn operand(&mut self, start: usize, end: usize) -> Option<Expr> {
        let expr = self.expr_at(start, end);
        matches!(
            expr,
            Expr::Var(_)
                | Expr::This
                | Expr::Literal(_)
                | Expr::Path(_)
                | Expr::Call(_)
                | Expr::EnumValue { .. }
                | Expr::List(_)
        )
        .then_some(expr)
    }

    /// `ATOM`, trying each verb occurrence left to right.
    fn atom(&mut self, start: usize, end: usize) -> Option<Predicate> {
        let (start, end) = self.trimmed(start, end);
        if let Some(pred) = self.attempt(|p| p.running_on(start, end)) {
            return Some(pred);
        }
        for pos in self.top_level(start, end) {
            for (word, verb, negated) in VERBS {
                if pos + word.len() > end || !self.lit(pos, word) {
                    continue;
                }
                let rest = pos + word.len();
                if let Some(pred) =
                    self.attempt(|p| p.atom_at(start, pos, rest, end, verb, negated))
                {
                    return Some(pred);
                }
            }
        }
        None
    }

    /// The atom with its subject at `start..verb_at` and the rest of the
    /// phrase at `rest..end`.
    fn atom_at(
        &mut self,
        start: usize,
        verb_at: usize,
        rest: usize,
        end: usize,
        verb: Verb,
        negated: bool,
    ) -> Option<Predicate> {
        match verb {
            Verb::Is => self.is_atom(start, verb_at, rest, end),
            Verb::Equals => {
                let lhs = self.operand(start, verb_at)?;
                let rhs = self.operand(rest, end)?;
                Some(Predicate::Compare {
                    lhs,
                    op: CmpOp::Eq,
                    rhs,
                    negated,
                })
            }
            Verb::Contains => {
                let container = self.operand(start, verb_at)?;
                let item = self.operand(rest, end)?;
                Some(Predicate::Contains {
                    container,
                    item,
                    negated,
                })
            }
            Verb::Has => {
                let name = self.attribute_name(rest, end)?;
                let element = self.operand(start, verb_at)?;
                Some(Predicate::HasAttribute {
                    element,
                    name,
                    negated,
                })
            }
            Verb::Exists => {
                let operand = self.operand(start, verb_at)?;
                if rest != end {
                    let place = self.keyword(rest, &[" in "])?;
                    self.operand(place, end)?;
                }
                Some(Predicate::Exists { operand, negated })
            }
        }
    }

    /// `(a|an) ⟦C⟧ attribute` spanning `pos..end`: the attribute name.
    fn attribute_name(&self, pos: usize, end: usize) -> Option<String> {
        let at = self.keyword(pos, &["a ", "an "])?;
        let (name, after) = match self.enc.placeholder(at) {
            Some((Placeholder::Link(link), after)) => {
                let text = &self.source.links[link].visible_text;
                let name = text.strip_prefix('`')?.strip_suffix('`')?;
                (name.to_string(), after)
            }
            _ => self.code(at)?,
        };
        (self.keyword(after, &[" attribute"]) == Some(end)).then_some(name)
    }

    /// An atom split at ` is `/` are `: a single subject with any `is`
    /// form, else `A and B` with `Holds`.
    fn is_atom(
        &mut self,
        start: usize,
        verb_at: usize,
        rest: usize,
        end: usize,
    ) -> Option<Predicate> {
        let single = self.attempt(|p| {
            let operand = p.operand(start, verb_at)?;
            p.is_tail(operand, rest, end)
        });
        if single.is_some() {
            return single;
        }
        let [(a_start, a_end), (b_start, b_end)] = self.split_top(start, verb_at, &[" and "])[..]
        else {
            return None;
        };
        let subjects = vec![self.operand(a_start, a_end)?, self.operand(b_start, b_end)?];
        self.holds(subjects, rest, end)
    }

    /// The forms after `OPERAND is `.
    fn is_tail(&mut self, operand: Expr, rest: usize, end: usize) -> Option<Predicate> {
        let (negated, at) = match self.keyword(rest, &["not "]) {
            Some(at) => (true, at),
            None => (false, rest),
        };
        let test = match &self.enc.text[at..end] {
            "non-null" if !negated => Some((Test::Null, true)),
            "empty" => Some((Test::Empty, negated)),
            "set" => Some((Test::Set, negated)),
            "unset" => Some((Test::Set, !negated)),
            _ => match self.literal_at(at, end) {
                Some(Literal::Null) => Some((Test::Null, negated)),
                Some(Literal::Bool(true)) => Some((Test::True, negated)),
                Some(Literal::Bool(false)) => Some((Test::False, negated)),
                _ => None,
            },
        };
        let test = test.or_else(|| {
            let type_at = self.keyword(at, &["a ", "an "])?;
            Some((Test::Type(self.type_test(type_at, end)?), negated))
        });
        if let Some((test, negated)) = test {
            return Some(Predicate::Is {
                operand,
                test,
                negated,
            });
        }
        if let Some(values) = self.attempt(|p| p.one_of(at, end)) {
            return Some(Predicate::OneOf {
                operand,
                values,
                negated,
            });
        }
        if let Some((values, neither)) = self.attempt(|p| p.value_alternatives(at, end)) {
            return Some(Predicate::OneOf {
                operand,
                values,
                negated: negated || neither,
            });
        }
        for (phrase, op) in COMPARISONS {
            if let Some(rhs_at) = self.keyword(at, &[phrase]).filter(|&rhs_at| rhs_at <= end) {
                let rhs = self.operand(rhs_at, end)?;
                return Some(Predicate::Compare {
                    lhs: operand,
                    op,
                    rhs,
                    negated,
                });
            }
        }
        if let Some(pred) = self.attempt(|p| p.holds(vec![operand.clone()], rest, end)) {
            return Some(pred);
        }
        let rhs = self.expr_at(at, end);
        let comparable = match &rhs {
            Expr::Literal(_) | Expr::Path(_) | Expr::Var(_) | Expr::EnumValue { .. } => true,
            Expr::List(items) => items.is_empty(),
            _ => false,
        };
        comparable.then_some(Predicate::Compare {
            lhs: operand,
            op: CmpOp::Eq,
            rhs,
            negated,
        })
    }

    /// `one of A, B(,)? or C`: the alternatives.
    fn one_of(&mut self, at: usize, end: usize) -> Option<Vec<Expr>> {
        let list = self.keyword(at, &["one of "])?;
        let parts = self.split_top(list, end, &[", or ", ", ", " or "]);
        if parts.len() < 2 {
            return None;
        }
        parts
            .into_iter()
            .map(|(start, end)| self.operand(start, end))
            .collect()
    }

    /// `(either |neither )?V (, | or | nor ) V …` with every `V` a literal or
    /// an enum value: the values and whether `neither` negates them.
    fn value_alternatives(&mut self, at: usize, end: usize) -> Option<(Vec<Expr>, bool)> {
        let (neither, list) = if let Some(list) = self.keyword(at, &["neither "]) {
            (true, list)
        } else {
            (false, self.keyword(at, &["either "]).unwrap_or(at))
        };
        let parts = self.split_top(list, end, &[", or ", ", nor ", ", ", " or ", " nor "]);
        if parts.len() < 2 {
            return None;
        }
        let values = parts
            .into_iter()
            .map(|(start, end)| {
                let value = self.expr_at(start, end);
                matches!(value, Expr::Literal(_) | Expr::EnumValue { .. }).then_some(value)
            })
            .collect::<Option<Vec<_>>>()?;
        Some((values, neither))
    }

    /// `TYPE` spanning `at..end`: a link or an IDL code identifier, either
    /// optionally followed by `object`/`element`/`node`, a primitive word,
    /// or an Infra word.
    fn type_test(&mut self, at: usize, end: usize) -> Option<TypeExpr> {
        let ends_type = |p: &Self, after: usize| {
            after == end
                || TYPE_WORDS
                    .iter()
                    .any(|word| after + word.len() == end && p.lit(after, word))
        };
        if let Some((Placeholder::Link(link), after)) = self.enc.placeholder(at) {
            if !ends_type(self, after) {
                return None;
            }
            let type_link = &self.source.links[link];
            let ty = TypeExpr::Nominal {
                ty: TypeRef::Unresolved(type_link.target.clone()?),
                text: type_link.visible_text.clone(),
            };
            self.set_role(link, LinkRole::Type);
            return Some(ty);
        }
        if let Some((name, after)) = self.code(at) {
            let idl = name.starts_with(|c: char| c.is_ascii_uppercase())
                && name.chars().all(|c| c.is_alphanumeric() || c == '_');
            return (idl && ends_type(self, after)).then(|| TypeExpr::Nominal {
                ty: TypeRef::Known(TypeKey::Idl(name.clone())),
                text: name,
            });
        }
        let text = &self.enc.text[at..end];
        primitive_phrase(text).map(TypeExpr::Primitive).or_else(|| {
            infra_word_kind(text).map(|kind| TypeExpr::Infra {
                kind,
                args: Vec::new(),
            })
        })
    }

    /// `(both )?(not )?⟦L⟧( (of|to|with) Z)?` spanning `rest..end`, the link
    /// being a predicate over `subjects`.
    fn holds(&mut self, subjects: Vec<Expr>, rest: usize, end: usize) -> Option<Predicate> {
        let at = self.keyword(rest, &["both "]).unwrap_or(rest);
        let (negated, at) = match self.keyword(at, &["not "]) {
            Some(at) => (true, at),
            None => (false, at),
        };
        let (Placeholder::Link(link), after) = self.enc.placeholder(at)? else {
            return None;
        };
        if self.is_this_link(link) || self.source.links[link].visible_text.starts_with('`') {
            return None;
        }
        if after != end {
            let object = self.keyword(after, &[" of ", " to ", " with "])?;
            self.operand(object, end)?;
        }
        let call = if self.link_callability(link) == Callability::Known(Callable::Predicate) {
            let receiver = match &subjects[..] {
                [subject] => subject.clone(),
                _ => Expr::List(subjects.clone()),
            };
            self.call_at(link, CallForm::Predicate, Some(receiver), end)
        } else {
            None
        };
        // Replaces the `Callee` role `call_at` gives the link.
        self.out
            .roles
            .insert(link, LinkRole::Predicate { call: call.clone() });
        let predicate_link = &self.source.links[link];
        Some(Predicate::Holds {
            subjects,
            link_id: predicate_link.id.clone(),
            target: predicate_link.target.clone(),
            call,
            negated,
        })
    }

    /// `(this|This) is running ` RUNCTX.
    fn running_on(&mut self, start: usize, end: usize) -> Option<Predicate> {
        let at = self.keyword(start, &["this is running ", "This is running "])?;
        let running_on = |context| Some(Predicate::RunningOn { context });
        if &self.enc.text[at..end] == "in parallel" {
            return running_on(RunContext::InParallel);
        }
        if let Some((Placeholder::Link(link), after)) = self.enc.placeholder(at) {
            if after == end && self.links_to_html(link, "in-parallel") {
                self.set_role(link, LinkRole::Keyword);
                return running_on(RunContext::InParallel);
            }
        }
        if let Some(queue_at) = self.keyword(at, &["on ", "within "]) {
            let queue = self.expr_at(queue_at, end);
            return matches!(queue, Expr::Path(_) | Expr::Var(_)).then_some(Predicate::RunningOn {
                context: RunContext::Queue(queue),
            });
        }
        // `as part of a task queued on X POSS ⟦relevant agent⟧ POSS ⟦event loop⟧`
        let task_on = self.keyword(at, &["as part of a task queued on "])?;
        let links: Vec<usize> = self.enc.links_in(task_on, end).collect();
        let [.., agent, event_loop] = links[..] else {
            return None;
        };
        if !self.links_to_html(agent, "relevant-agent")
            || !self.links_to_html(event_loop, "concept-agent-event-loop")
        {
            return None;
        }
        let agent_start = self.enc.to_enc(self.source.links[agent].span.start);
        let loop_start = self.enc.to_enc(self.source.links[event_loop].span.start);
        let (_, agent_end) = self.enc.placeholder(agent_start)?;
        let (_, loop_end) = self.enc.placeholder(loop_start)?;
        let possessive = |from: usize, to: usize| {
            POSS.iter()
                .any(|poss| from + poss.len() == to && self.lit(from, poss))
        };
        if loop_end != end || !possessive(agent_end, loop_start) {
            return None;
        }
        let x_end = POSS
            .iter()
            .find(|poss| {
                agent_start >= task_on + poss.len()
                    && possessive(agent_start - poss.len(), agent_start)
            })
            .map(|poss| agent_start - poss.len())?;
        let task_queue_owner = self.operand(task_on, x_end)?;
        self.set_role(agent, LinkRole::Keyword);
        self.set_role(event_loop, LinkRole::Keyword);
        running_on(RunContext::EventLoopTask(task_queue_owner))
    }

    fn links_to_html(&self, link: usize, anchor: &str) -> bool {
        self.source.links[link]
            .target
            .as_ref()
            .is_some_and(|target| {
                target.spec.eq_ignore_ascii_case("HTML") && target.anchor == anchor
            })
    }
}

#[cfg(test)]
mod tests {
    use crate::state::grammar::{Callable, Encoded, Env, Parser};
    use crate::state::ir::{
        tests_support::sources, CallForm, CmpOp, Expr, Hop, LinkRole, Path, Predicate, Root,
        RunContext, Test,
    };
    use crate::state::model::{Literal, TypeExpr, TypeKey, TypeRef};
    use crate::state::testing::var;

    fn pred_with(text: &str, env: &Env) -> (Predicate, Vec<crate::state::ir::Call>) {
        let src = sources(&[text]).remove(0);
        let enc = Encoded::new(&src);
        let mut p = Parser::new(&enc, &src, env);
        let end = enc.text.trim_end_matches('.').len();
        let pred = p.predicate_at(0, end);
        (pred, p.out.calls.clone())
    }
    fn pred(text: &str) -> Predicate {
        pred_with(text, &Env::default()).0
    }
    fn field_path<'a>(root: &'a str, visible: &'a str) -> impl Fn(&Expr) -> bool + 'a {
        move |e| matches!(e, Expr::Path(Path { root: Root::Var(r), hops, .. }) if r == root && matches!(&hops[..], [Hop::Field { visible_text, .. }] if visible_text == visible))
    }

    #[test]
    fn g5_non_null_assertion_and_queue_context() {
        let Predicate::Is {
            operand,
            test: Test::Null,
            negated: true,
        } = pred("<var>document</var>'s <a href=\"#concept-document-about-base-url\">about base URL</a> is non-null")
        else {
            panic!()
        };
        assert!(field_path("document", "about base URL")(&operand));
        let Predicate::RunningOn {
            context: RunContext::Queue(q),
        } = pred("this is running on <var>traversable</var>'s <a href=\"#tn-session-history-traversal-queue\">session history traversal queue</a>")
        else {
            panic!()
        };
        assert!(field_path("traversable", "session history traversal queue")(&q));
        assert!(matches!(pred("<var>element</var> is an <code>input</code> element whose <code>type</code> attribute is in the Color state"), Predicate::Opaque { .. }));
    }

    #[test]
    fn run_contexts_capital_this_and_linked_in_parallel() {
        assert_eq!(
            pred("This is running <a href=\"#in-parallel\">in parallel</a>"),
            Predicate::RunningOn {
                context: RunContext::InParallel
            }
        );
        assert_eq!(
            pred("this is running in parallel"),
            Predicate::RunningOn {
                context: RunContext::InParallel
            }
        );
        let Predicate::RunningOn {
            context: RunContext::EventLoopTask(x),
        } = pred("this is running as part of a task queued on <var>document</var>'s <a href=\"#relevant-agent\">relevant agent</a>'s <a href=\"#concept-agent-event-loop\">event loop</a>")
        else {
            panic!()
        };
        assert_eq!(x, var("document"));
    }

    #[test]
    fn tests_comparisons_and_sets() {
        assert_eq!(
            pred("<var>x</var> is null"),
            Predicate::Is {
                operand: var("x"),
                test: Test::Null,
                negated: false
            }
        );
        assert_eq!(
            pred("<var>x</var> is not true"),
            Predicate::Is {
                operand: var("x"),
                test: Test::True,
                negated: true
            }
        );
        assert!(matches!(
            pred("<a href=\"https://webidl.spec.whatwg.org/#this\">this</a>’s <a href=\"#dispatch-flag\">dispatch flag</a> is set"),
            Predicate::Is {
                test: Test::Set,
                negated: false,
                ..
            }
        ));
        assert!(matches!(
            pred("<var>f</var> is unset"),
            Predicate::Is {
                test: Test::Set,
                negated: true,
                ..
            }
        ));
        assert_eq!(
            pred("<var>n</var> is greater than or equal to 2"),
            Predicate::Compare {
                lhs: var("n"),
                op: CmpOp::Ge,
                rhs: Expr::Literal(Literal::Number("2".into())),
                negated: false
            }
        );
        assert_eq!(
            pred("<var>i</var> is −1"),
            Predicate::Compare {
                lhs: var("i"),
                op: CmpOp::Eq,
                rhs: Expr::Literal(Literal::Number("-1".into())),
                negated: false
            }
        );
        assert_eq!(
            pred("<var>url</var> is failure"),
            Predicate::Compare {
                lhs: var("url"),
                op: CmpOp::Eq,
                rhs: Expr::Literal(Literal::Failure),
                negated: false
            }
        );
        assert_eq!(
            pred("<var>a</var> equals <var>b</var>"),
            Predicate::Compare {
                lhs: var("a"),
                op: CmpOp::Eq,
                rhs: var("b"),
                negated: false
            }
        );
        assert!(matches!(
            pred("<var>list</var> contains <var>item</var>"),
            Predicate::Contains { negated: false, .. }
        ));
        assert!(matches!(
            pred("<var>map</var>[<var>key</var>] exists"),
            Predicate::Exists { negated: false, .. }
        ));
        assert_eq!(
            pred("<var>element</var> has a <code>download</code> attribute"),
            Predicate::HasAttribute {
                element: var("element"),
                name: "download".into(),
                negated: false
            }
        );
        let Predicate::OneOf { values, .. } =
            pred("<var>mode</var> is \"<code>a</code>\" or \"<code>b</code>\"")
        else {
            panic!()
        };
        assert_eq!(
            values,
            vec![
                Expr::EnumValue {
                    text: "a".into(),
                    target: None
                },
                Expr::EnumValue {
                    text: "b".into(),
                    target: None
                }
            ]
        );
        assert!(matches!(
            pred("<var>node</var> is a <a href=\"#concept-document\">document</a>"),
            Predicate::Is {
                test: Test::Type(_),
                ..
            }
        ));
    }

    #[test]
    fn conjunctions_disjunctions_implications_and_all_or_nothing() {
        assert!(
            matches!(pred("<var>a</var> is null and <var>b</var> is true"), Predicate::And(v) if v.len() == 2)
        );
        assert!(
            matches!(pred("<var>a</var> is null, or <var>b</var> is true"), Predicate::Or(v) if v.len() == 2)
        );
        assert!(matches!(
            pred("if <var>a</var> is null, then <var>b</var> is true"),
            Predicate::Implies(..)
        ));
        assert!(matches!(
            pred("<var>a</var> is null and <var>usability</var> is good"),
            Predicate::Opaque { .. }
        ));
        let Predicate::Or(parts) =
            pred("<var>a</var> is null or <var>b</var> is greater than or equal to <var>c</var>")
        else {
            panic!()
        };
        assert_eq!(
            parts[1],
            Predicate::Compare {
                lhs: var("b"),
                op: CmpOp::Ge,
                rhs: var("c"),
                negated: false
            }
        );
    }

    #[test]
    fn holds_with_and_without_a_predicate_callee() {
        let Predicate::Holds {
            subjects,
            target,
            call: None,
            ..
        } = pred("<var>parentDoc</var> is <a href=\"#fully-active\">fully active</a>")
        else {
            panic!()
        };
        assert_eq!(
            (subjects, target.unwrap().anchor),
            (vec![var("parentDoc")], "fully-active".to_string())
        );
        let env = Env {
            spec: "HTML".into(),
            callables: [("same-origin".to_string(), Callable::Predicate)].into(),
            ..Env::default()
        };
        let (p, calls) = pred_with(
            "<var>A</var> and <var>B</var> are <a href=\"#same-origin\">same origin</a>",
            &env,
        );
        let Predicate::Holds {
            subjects,
            call: Some(id),
            ..
        } = p
        else {
            panic!()
        };
        assert_eq!(subjects, vec![var("A"), var("B")]);
        assert_eq!(
            (
                calls[0].id.clone(),
                calls[0].form,
                calls[0].receiver.clone()
            ),
            (
                id,
                CallForm::Predicate,
                Some(Expr::List(vec![var("A"), var("B")]))
            )
        );
        let _ = LinkRole::Keyword;
    }

    #[test]
    fn remaining_forms_negations_and_roles() {
        assert!(matches!(
            pred("<var>doc</var> is not <a href=\"#fully-active\">fully active</a>"),
            Predicate::Holds { negated: true, .. }
        ));
        assert!(matches!(
            pred("<var>x</var> is a <code>Document</code> object"),
            Predicate::Is { test: Test::Type(TypeExpr::Nominal { ty: TypeRef::Known(TypeKey::Idl(name)), .. }), .. } if name == "Document"
        ));
        assert!(matches!(
            pred("<var>x</var> is not a list"),
            Predicate::Is {
                test: Test::Type(TypeExpr::Infra { .. }),
                negated: true,
                ..
            }
        ));
        assert!(
            matches!(pred("<var>x</var> is one of <var>a</var>, <var>b</var>, or <var>c</var>"), Predicate::OneOf { values, negated: false, .. } if values.len() == 3)
        );
        assert!(
            matches!(pred("<var>x</var> is neither null nor <code>undefined</code>"), Predicate::OneOf { values, negated: true, .. } if values.len() == 2)
        );
        assert!(matches!(
            pred("<var>e</var> does not have an <code>href</code> attribute"),
            Predicate::HasAttribute { negated: true, .. }
        ));
        assert!(matches!(
            pred("<var>k</var> does not exist in <var>map</var>"),
            Predicate::Exists { negated: true, .. }
        ));
        assert!(matches!(
            pred("<var>n</var> is not less than <var>m</var>"),
            Predicate::Compare {
                op: CmpOp::Lt,
                negated: true,
                ..
            }
        ));

        let env = Env {
            spec: "HTML".into(),
            callables: [("same-origin".to_string(), Callable::Predicate)].into(),
            ..Env::default()
        };
        let src = sources(&[
            "<var>A</var> is <a href=\"#same-origin\">same origin</a> with <var>B</var>",
        ])
        .remove(0);
        let enc = Encoded::new(&src);
        let mut p = Parser::new(&enc, &src, &env);
        let Predicate::Holds { call: Some(id), .. } = p.predicate_at(0, enc.text.len()) else {
            panic!()
        };
        assert_eq!(p.out.roles[&0], LinkRole::Predicate { call: Some(id) });
        assert_eq!(p.out.calls[0].receiver, Some(var("A")));

        let src = sources(&["<var>d</var>'s <a href=\"#url\">URL</a> is <a href=\"#fully-active\">fully active</a> and the moon is round"]).remove(0);
        let enc = Encoded::new(&src);
        let mut p = Parser::new(&enc, &src, &env);
        assert!(matches!(
            p.predicate_at(0, enc.text.len()),
            Predicate::Opaque { .. }
        ));
        assert!(p.out.roles.is_empty() && p.out.calls.is_empty());
    }

    #[test]
    fn conditional_values() {
        let src = sources(&["\"<code>form-submission</code>\" if <var>exceptionsEnabled</var> is true; otherwise \"<code>other</code>\""]).remove(0);
        let enc = Encoded::new(&src);
        let env = Env::default();
        let mut p = Parser::new(&enc, &src, &env);
        let Expr::Conditional {
            condition,
            then,
            otherwise,
        } = p.expr_at(0, enc.text.len())
        else {
            panic!()
        };
        assert_eq!(
            *condition,
            Predicate::Is {
                operand: var("exceptionsEnabled"),
                test: Test::True,
                negated: false
            }
        );
        assert_eq!(
            (*then, *otherwise),
            (
                Expr::EnumValue {
                    text: "form-submission".into(),
                    target: None
                },
                Expr::EnumValue {
                    text: "other".into(),
                    target: None
                }
            )
        );
    }
}
