//! Control-flow heads (§8.2): conditions, loops, exits, waits, parallel
//! blocks and assertions, with the inline blocks they open.
use crate::state::grammar::{Callability, Parser, Placeholder};
use crate::state::ir::{
    BlockRole, BodyLoc, ExceptionRef, Expr, LinkRole, LoopForm, Predicate, SourceContext,
    StatementKind, StatementParent,
};

/// Words after `, ` that end an `If` condition and start its inline block.
const VERBHEADS: [&str; 14] = [
    "set",
    "let",
    "return",
    "append",
    "remove",
    "throw",
    "abort",
    "continue",
    "run",
    "fire",
    "queue",
    "optionally",
    "increment",
    "decrement",
];
const QUOTES: [(&str, &str); 2] = [("\"", "\""), ("\u{201C}", "\u{201D}")];
const ASSERT: (&str, &str) = ("INFRA", "assert");
const LIST_ITERATE: (&str, &str) = ("INFRA", "list-iterate");
const MAP_ITERATE: (&str, &str) = ("INFRA", "map-iterate");
const ITERATION_CONTINUE: (&str, &str) = ("INFRA", "iteration-continue");
const ITERATION_BREAK: (&str, &str) = ("INFRA", "iteration-break");
const IN_PARALLEL: (&str, &str) = ("HTML", "in-parallel");
const THROW: (&str, &str) = ("WEBIDL", "dfn-throw");
/// Anchors whose links spell a control keyword, never a call.
const CONTROL_KEYWORDS: [(&str, &str); 7] = [
    THROW,
    LIST_ITERATE,
    MAP_ITERATE,
    ITERATION_CONTINUE,
    ITERATION_BREAK,
    ASSERT,
    IN_PARALLEL,
];

/// A parsed `For each` head.
struct LoopHead {
    vars: Vec<String>,
    collection: Expr,
    filter: Option<Predicate>,
    order: Option<String>,
    /// Where the head's terminator (`,` or the final `:`) is.
    terminator: usize,
}

impl Parser<'_> {
    /// A control-flow head at clause start `at` (§8.2). At the start of the
    /// source, a leading `⌛ ` and `Optionally` are skipped first.
    pub(crate) fn try_control(&mut self, at: usize) -> bool {
        let pos = if at == 0 {
            self.after_step_prefix()
        } else {
            at
        };
        self.try_if(at, pos)
            || self.try_otherwise(at, pos)
            || self.try_for_each(at, pos)
            || self.try_while(at, pos)
            || self.try_return(at, pos)
            || self.try_throw(at, pos)
            || self.try_abort(at, pos)
            || self.try_continue_or_break(at, pos)
            || self.try_wait(at, pos)
            || self.try_in_parallel(at, pos)
            || self.try_run_steps(at, pos)
            || self.try_assert(at, pos)
    }

    fn after_step_prefix(&self) -> usize {
        let pos = self.keyword(0, &["\u{231B} "]).unwrap_or(0);
        self.keyword(pos, &["Optionally, ", "Optionally "])
            .unwrap_or(pos)
    }

    fn text_end(&self) -> usize {
        self.enc.text.trim_end().len()
    }

    fn is_final_colon(&self, pos: usize) -> bool {
        self.lit(pos, ":") && pos + 1 == self.text_end()
    }

    /// The first of `words` at `pos` as a whole word; the position after it.
    fn head_word(&self, pos: usize, words: &[&str]) -> Option<usize> {
        words
            .iter()
            .find(|word| self.lit(pos, word) && !self.word_continues(pos + word.len()))
            .map(|word| pos + word.len())
    }

    /// A link at `pos` to one of `anchors`: its index and the position after
    /// it.
    fn head_link(&self, pos: usize, anchors: &[(&str, &str)]) -> Option<(usize, usize)> {
        let (Placeholder::Link(link), after) = self.enc.placeholder(pos)? else {
            return None;
        };
        let target = self.source.links[link].target.as_ref()?;
        anchors
            .iter()
            .any(|(spec, anchor)| {
                target.spec.eq_ignore_ascii_case(spec) && target.anchor == *anchor
            })
            .then_some((link, after))
    }

    /// A link at `pos` that spells a control keyword (`throw`, `For each`,
    /// `continue`, `break`, `Assert`, `in parallel`).
    pub(crate) fn is_control_keyword_link(&self, pos: usize) -> bool {
        self.head_link(pos, &CONTROL_KEYWORDS).is_some()
    }

    /// A head spelled by one of `words` or by a link to one of `anchors`:
    /// the head link, if any, and the position after the head.
    fn head(
        &self,
        pos: usize,
        words: &[&str],
        anchors: &[(&str, &str)],
    ) -> Option<(Option<usize>, usize)> {
        if let Some(after) = self.head_word(pos, words) {
            return Some((None, after));
        }
        self.head_link(pos, anchors)
            .map(|(link, after)| (Some(link), after))
    }

    fn keyword_role(&mut self, link: Option<usize>) {
        if let Some(link) = link {
            self.set_role(link, LinkRole::Keyword);
        }
    }

    /// The block of a head ending in `:`: the step's child steps.
    fn child_body(&self) -> BodyLoc {
        match &self.source.context {
            SourceContext::Algorithm {
                step_id: Some(step_id),
                ..
            } => BodyLoc::ChildSteps {
                step_id: step_id.clone(),
            },
            _ => BodyLoc::InlineRest,
        }
    }

    /// The body of a head whose terminator is at `terminator` (a `,`, a `: `
    /// before an inline body, or the final `:`) and where the clauses after
    /// it start.
    fn body_after(&self, terminator: usize) -> (BodyLoc, usize) {
        if self.is_final_colon(terminator) {
            return (self.child_body(), self.text_end());
        }
        let rest = self
            .keyword(terminator, &[", ", ",", ": "])
            .unwrap_or(terminator);
        (BodyLoc::InlineRest, rest)
    }

    /// The first top-level `,`, `: ` or final `:` in `start..`.
    fn head_terminator(&self, start: usize) -> Option<usize> {
        self.top_level(start, self.text_end())
            .into_iter()
            .find(|&pos| self.lit(pos, ",") || self.lit(pos, ": ") || self.is_final_colon(pos))
    }

    /// Pushes a control statement; an inline `body` makes it the parent of
    /// the statements that follow, in block `role`.
    fn push_block(
        &mut self,
        start: usize,
        end: usize,
        kind_name: &str,
        kind: StatementKind,
        inline_role: Option<BlockRole>,
    ) -> String {
        let id = self.push(start, end, kind_name, kind);
        if let Some(role) = inline_role {
            self.inline_parent = Some(StatementParent {
                statement_id: id.clone(),
                role,
            });
        }
        id
    }

    /// Where a condition starting at `start` ends: at the first top-level
    /// `, then `, `, ` before a `VERBHEAD`, or final `:`. Returns the
    /// condition end, where the clauses after it start, and whether the head
    /// ends in `:`.
    fn condition_end(&self, start: usize) -> Option<(usize, usize, bool)> {
        let end = self.text_end();
        self.top_level(start, end).into_iter().find_map(|pos| {
            if self.lit(pos, ", then:") && pos + ", then:".len() == end {
                return Some((pos, end, true));
            }
            if self.lit(pos, ", then ") {
                return Some((pos, pos + ", then ".len(), false));
            }
            if self.lit(pos, ", ") && self.is_verb_head(pos + ", ".len()) {
                return Some((pos, pos + ", ".len(), false));
            }
            self.is_final_colon(pos).then_some((pos, end, true))
        })
    }

    /// `VERBHEAD` at `pos`: a verb of `VERBHEADS`, a callable link, an Infra
    /// operation link, or a link to `continue`/`break`.
    pub(crate) fn is_verb_head(&self, pos: usize) -> bool {
        if let Some((Placeholder::Link(link), _)) = self.enc.placeholder(pos) {
            return self.link_callability(link) != Callability::No
                || self.infra_op(pos).is_some()
                || self
                    .head_link(pos, &[ITERATION_CONTINUE, ITERATION_BREAK])
                    .is_some();
        }
        self.head_word(pos, &VERBHEADS).is_some()
    }

    /// `If ` PRED, then `, then `, `, ` VERBHEAD, or a final `:`.
    fn try_if(&mut self, at: usize, pos: usize) -> bool {
        let Some(start) = self.keyword(pos, &["If "]) else {
            return false;
        };
        let Some((end, rest, colon)) = self.condition_end(start) else {
            return false;
        };
        let condition = self.predicate_at(start, end);
        let then = if colon {
            self.child_body()
        } else {
            BodyLoc::InlineRest
        };
        let role = (then == BodyLoc::InlineRest).then_some(BlockRole::Then);
        self.push_block(at, end, "if", StatementKind::If { condition, then }, role);
        self.covered_until = rest;
        true
    }

    /// `Otherwise`/`Else` (`,`|`:`)? (` if ` PRED (`, then `|`:`))?: a
    /// sibling of the source's last `If`, whose parent it shares. Without
    /// that `If`, a lowercase one inside the source belongs to a value
    /// ("true if P; otherwise false").
    pub(crate) fn try_otherwise(&mut self, at: usize, pos: usize) -> bool {
        let Some(word_end) = self.head_word(pos, &["Otherwise", "otherwise", "Else", "else"])
        else {
            return false;
        };
        let previous_if = self
            .out
            .statements
            .iter()
            .rev()
            .find(|s| matches!(s.kind, StatementKind::If { .. }))
            .map(|s| (s.id.clone(), s.parent.clone()));
        let lowercase = self.enc.text[pos..].starts_with(char::is_lowercase);
        if previous_if.is_none() && at != 0 && lowercase {
            return false;
        }
        let after = self.keyword(word_end, &[",", ":"]).unwrap_or(word_end);
        let (condition, end, rest, colon) = match self.keyword(after, &[" if "]) {
            Some(start) => {
                let Some((end, rest, colon)) = self.condition_end(start) else {
                    return false;
                };
                (Some(self.predicate_at(start, end)), end, rest, colon)
            }
            None if self.is_final_colon(word_end) => (None, word_end, self.text_end(), true),
            None => {
                let rest = self.keyword(after, &[" "]).unwrap_or(after);
                (None, word_end, rest, false)
            }
        };
        let body = if colon {
            self.child_body()
        } else {
            BodyLoc::InlineRest
        };
        let of = previous_if.map(|(id, parent)| {
            self.inline_parent = parent;
            id
        });
        let role = (body == BodyLoc::InlineRest).then_some(BlockRole::Otherwise);
        let kind = StatementKind::Otherwise {
            of,
            condition,
            body,
        };
        self.push_block(at, end, "otherwise", kind, role);
        self.covered_until = rest;
        true
    }

    /// `For each ` TYPEPH? `⟦V⟧` (` → ⟦V⟧`)? ((` of `|` in `|` from `) EXPR)?
    /// (` whose ` PRED | ` that ` PRED)? (`, in ` ORDER)? (`,`|`:`).
    fn try_for_each(&mut self, at: usize, pos: usize) -> bool {
        let (head_link, start) = match self.keyword(pos, &["For each "]) {
            Some(start) => (None, start),
            None => match self
                .head_link(pos, &[LIST_ITERATE, MAP_ITERATE])
                .and_then(|(link, after)| Some((Some(link), self.keyword(after, &[" "])?)))
            {
                Some(head) => head,
                None => return false,
            },
        };
        let Some((type_links, head)) = self.loop_head(start) else {
            return false;
        };
        let (body, rest) = self.body_after(head.terminator);
        let role = (body == BodyLoc::InlineRest).then_some(BlockRole::Body);
        let kind = StatementKind::ForEach {
            vars: head.vars,
            collection: head.collection,
            filter: head.filter,
            order: head.order,
            body,
        };
        self.push_block(at, head.terminator, "for_each", kind, role);
        self.keyword_role(head_link);
        for link in type_links {
            self.set_role(link, LinkRole::Type);
        }
        self.covered_until = rest;
        true
    }

    /// The `For each` head after `For each ` at `start`, with the links of
    /// its type phrase. A loop without a collection must name a type; its
    /// collection is that type phrase, opaque.
    fn loop_head(&mut self, start: usize) -> Option<(Vec<usize>, LoopHead)> {
        let (type_links, var_start, vars, end) = match self.tuple_vars(start) {
            Some((vars, end)) => (Vec::new(), start, vars, end),
            None => {
                let (type_links, var_start, var, mut end) = self.loop_var(start)?;
                let mut vars = vec![self.enc.vars[var].clone()];
                if let Some((Placeholder::Var(second), after)) = self
                    .keyword(end, &[" \u{2192} "])
                    .and_then(|next| self.enc.placeholder(next))
                {
                    vars.push(self.enc.vars[second].clone());
                    end = after;
                }
                (type_links, var_start, vars, end)
            }
        };
        let collection_start = self.keyword(end, &[" of ", " in ", " from "]);
        if collection_start.is_none() && var_start == start {
            return None;
        }
        let terminator = self.head_terminator(end)?;
        let (region_end, order, terminator) = match self.keyword(terminator, &[", in "]) {
            Some(order_start) => {
                let order_end = self.head_terminator(order_start)?;
                (terminator, Some((order_start, order_end)), order_end)
            }
            None => (terminator, None, terminator),
        };
        let filter_at = self
            .top_level(collection_start.unwrap_or(end), region_end)
            .into_iter()
            .find_map(|pos| Some((pos, self.keyword(pos, &[" whose ", " that "])?)));
        let collection_end = filter_at.map_or(region_end, |(pos, _)| pos);
        let collection = match collection_start {
            Some(collection_start) => self.expr_at(collection_start, collection_end),
            None if collection_end == end => Expr::Opaque {
                text: self.src_text(start, var_start).trim().to_string(),
            },
            None => return None,
        };
        let filter = filter_at.map(|(_, filter_start)| self.predicate_at(filter_start, region_end));
        let head = LoopHead {
            vars,
            collection,
            filter,
            order: order.map(|(start, end)| self.src_text(start, end).trim().to_string()),
            terminator,
        };
        Some((type_links, head))
    }

    /// `(⟦V⟧, ⟦V⟧…)` at `pos`: the variable names and the position after
    /// `)`.
    fn tuple_vars(&self, pos: usize) -> Option<(Vec<String>, usize)> {
        let mut at = self.keyword(pos, &["("])?;
        let mut vars = Vec::new();
        loop {
            let (Placeholder::Var(var), end) = self.enc.placeholder(at)? else {
                return None;
            };
            vars.push(self.enc.vars[var].clone());
            if let Some(close) = self.keyword(end, &[")"]) {
                return (vars.len() > 1).then_some((vars, close));
            }
            at = self.keyword(end, &[", "])?;
        }
    }

    /// Up to four words or a code/link type phrase, then `⟦V⟧`, at `pos`:
    /// the phrase's links, the variable's start, index and end.
    fn loop_var(&self, pos: usize) -> Option<(Vec<usize>, usize, usize, usize)> {
        let mut at = pos;
        let mut links = Vec::new();
        for _ in 0..=4 {
            match self.enc.placeholder(at) {
                Some((Placeholder::Var(var), end)) => return Some((links, at, var, end)),
                Some((Placeholder::Link(link), _)) => links.push(link),
                None => {}
            }
            let word_end = self.word_end(at)?;
            at = self.keyword(word_end, &[" "])?;
        }
        None
    }

    /// `While ` PRED (`,`|`:`), `Repeat` (`,`|`:`)?, or `Loop:`.
    fn try_while(&mut self, at: usize, pos: usize) -> bool {
        let text_end = self.text_end();
        let (condition, form, terminator) = if let Some(start) = self.keyword(pos, &["While "]) {
            let Some(terminator) = self.head_terminator(start) else {
                return false;
            };
            let condition = self.predicate_at(start, terminator);
            (Some(condition), LoopForm::While, terminator)
        } else if let Some(after) = self.head_word(pos, &["Repeat"]) {
            if !(self.lit(after, ",") || self.is_final_colon(after) || after == text_end) {
                return false;
            }
            (None, LoopForm::Repeat, after)
        } else if self.lit(pos, "Loop:") && self.is_final_colon(pos + "Loop".len()) {
            (None, LoopForm::Repeat, pos + "Loop".len())
        } else {
            return false;
        };
        let (body, rest) = if terminator == text_end {
            (self.child_body(), text_end)
        } else {
            self.body_after(terminator)
        };
        let role = (body == BodyLoc::InlineRest).then_some(BlockRole::Body);
        let kind = StatementKind::While {
            condition,
            form,
            body,
        };
        self.push_block(at, terminator, "while", kind, role);
        self.covered_until = rest;
        true
    }

    /// `Return` (` ` EXPR)? END.
    fn try_return(&mut self, at: usize, pos: usize) -> bool {
        let Some(after) = self.head_word(pos, &["Return", "return"]) else {
            return false;
        };
        let (value, end) = if self.is_end(after) {
            (None, after)
        } else if let Some(start) = self.keyword(after, &[" "]) {
            let end = self.statement_value_end(start, None);
            (Some(self.expr_at(start, end)), end)
        } else {
            return false;
        };
        self.push(at, end, "return", StatementKind::Return { value });
        self.covered_until = end;
        true
    }

    /// (`Throw`|`⟦WEBIDL#dfn-throw⟧`) ` ` (`a`|`an`)? (quoted code)? `⟦L⟧`
    /// END.
    fn try_throw(&mut self, at: usize, pos: usize) -> bool {
        let Some((head_link, exception, links, end)) = self.throw_head(pos) else {
            return false;
        };
        self.push(at, end, "throw", StatementKind::Throw { exception });
        self.keyword_role(head_link);
        for link in links {
            self.set_role(link, LinkRole::Type);
        }
        self.covered_until = end;
        true
    }

    fn throw_head(&self, pos: usize) -> Option<(Option<usize>, ExceptionRef, Vec<usize>, usize)> {
        let (head_link, start) = self
            .head(pos, &["Throw", "throw"], &[THROW])
            .and_then(|(link, after)| Some((link, self.keyword(after, &[" "])?)))?;
        let mut at = self.keyword(start, &["a ", "an "]).unwrap_or(start);
        let mut links = Vec::new();
        let mut name = None;
        if let Some((open, close)) = QUOTES.iter().find(|(open, _)| self.lit(at, open)) {
            let inner = at + open.len();
            let (text, inner_end) = match self.enc.placeholder(inner) {
                Some((Placeholder::Link(link), end)) => {
                    links.push(link);
                    (self.source.links[link].visible_text.replace('`', ""), end)
                }
                _ => self.code(inner)?,
            };
            let closed = self.keyword(inner_end, &[close])?;
            at = self.keyword(closed, &[" "])?;
            name = Some(text);
        }
        let (Placeholder::Link(link), end) = self.enc.placeholder(at)? else {
            return None;
        };
        if !self.is_end(end) {
            return None;
        }
        links.push(link);
        let exception_link = &self.source.links[link];
        let exception = ExceptionRef {
            name: name.or_else(|| Some(exception_link.visible_text.replace('`', ""))),
            link: exception_link.target.clone(),
            text: self.src_text(start, end),
        };
        Some((head_link, exception, links, end))
    }

    /// (`Abort`|`Terminate`) ` ` text END.
    fn try_abort(&mut self, at: usize, pos: usize) -> bool {
        let Some(start) = self
            .head_word(pos, &["Abort", "abort", "Terminate", "terminate"])
            .and_then(|after| self.keyword(after, &[" "]))
        else {
            return false;
        };
        let end = self.value_end(start, None);
        if end <= start || !self.is_end(end) {
            return false;
        }
        let text = self.src_text(start, end);
        self.push(at, end, "abort", StatementKind::Abort { text });
        self.covered_until = end;
        true
    }

    /// `Continue`/`Break`, as a word or an Infra link, then (` to the next `
    /// text)? END.
    fn try_continue_or_break(&mut self, at: usize, pos: usize) -> bool {
        let (kind_name, kind, (link, mut end)) =
            if let Some(head) = self.head(pos, &["Continue", "continue"], &[ITERATION_CONTINUE]) {
                ("continue", StatementKind::Continue, head)
            } else if let Some(head) = self.head(pos, &["Break", "break"], &[ITERATION_BREAK]) {
                ("break", StatementKind::Break, head)
            } else {
                return false;
            };
        if let Some(target) = self.keyword(end, &[" to the next "]) {
            end = self.value_end(target, None);
        }
        if !self.is_end(end) {
            return false;
        }
        self.push(at, end, kind_name, kind);
        self.keyword_role(link);
        self.covered_until = end;
        true
    }

    /// `Wait` (` until ` PRED | ` for ` text)? END.
    fn try_wait(&mut self, at: usize, pos: usize) -> bool {
        let Some(after) = self.head_word(pos, &["Wait"]) else {
            return false;
        };
        let (condition, end) = if let Some(start) = self.keyword(after, &[" until "]) {
            let end = self.value_end(start, None);
            (Some(self.predicate_at(start, end)), end)
        } else if let Some(start) = self.keyword(after, &[" for "]) {
            (None, self.value_end(start, None))
        } else if self.is_end(after) {
            (None, after)
        } else {
            return false;
        };
        let text = self.src_text(pos, end);
        self.push(at, end, "wait", StatementKind::Wait { condition, text });
        self.covered_until = end;
        true
    }

    /// (`Run the following steps `|`Run these steps `)? `in parallel`, or
    /// `In parallel`, then `:` or `, `.
    fn try_in_parallel(&mut self, at: usize, pos: usize) -> bool {
        let start = self
            .keyword(pos, &["Run the following steps ", "Run these steps "])
            .unwrap_or(pos);
        let Some((link, terminator)) =
            self.head(start, &["In parallel", "in parallel"], &[IN_PARALLEL])
        else {
            return false;
        };
        if !(self.is_final_colon(terminator) || self.lit(terminator, ", ")) {
            return false;
        }
        let (body, rest) = self.body_after(terminator);
        let role = (body == BodyLoc::InlineRest).then_some(BlockRole::Body);
        let kind = StatementKind::InParallel { body };
        self.push_block(at, terminator, "in_parallel", kind, role);
        self.keyword_role(link);
        self.covered_until = rest;
        true
    }

    /// `Run the following (sub)?steps:`.
    fn try_run_steps(&mut self, at: usize, pos: usize) -> bool {
        let Some(end) = self.keyword(
            pos,
            &["Run the following steps", "Run the following substeps"],
        ) else {
            return false;
        };
        if !self.is_final_colon(end) {
            return false;
        }
        let body = self.child_body();
        self.push(at, end, "run_steps", StatementKind::RunSteps { body });
        self.covered_until = self.text_end();
        true
    }

    /// (`Assert`|`⟦INFRA#assert⟧`) `: ` (`the following is true: `|`that `)?
    /// PRED `.`?; the predicate runs to the end of the sentence.
    fn try_assert(&mut self, at: usize, pos: usize) -> bool {
        let Some((link, start)) = self
            .head(pos, &["Assert"], &[ASSERT])
            .and_then(|(link, after)| Some((link, self.keyword(after, &[": "])?)))
        else {
            return false;
        };
        let start = self
            .keyword(start, &["the following is true: ", "that "])
            .unwrap_or(start);
        let end = self.sentence_end(start);
        let predicate = self.predicate_at(start, end);
        self.push(at, end, "assert", StatementKind::Assert { predicate });
        self.keyword_role(link);
        self.covered_until = end;
        true
    }

    /// The first top-level `. ` or `;` after `start`, else the text end
    /// without a final `.`.
    fn sentence_end(&self, start: usize) -> usize {
        let text_end = self.text_end();
        self.top_level(start, text_end)
            .into_iter()
            .find(|&pos| self.lit(pos, ". ") || self.lit(pos, ";"))
            .unwrap_or_else(|| {
                if self.enc.text[..text_end].ends_with('.') {
                    text_end - 1
                } else {
                    text_end
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use crate::state::grammar::Env;
    use crate::state::ir::{
        parse_source_with, tests_support::sources, BlockRole, BodyLoc, Expr, LoopForm,
        ParsedSource, Predicate, Statement, StatementKind, Test,
    };
    use crate::state::testing::var;

    fn parse(step: &str) -> ParsedSource {
        parse_source_with(&sources(&[step]).remove(0), &Env::default())
    }
    /// Parses in DOM, where links to other specs are callable.
    fn parse_in_dom(step: &str) -> ParsedSource {
        let env = Env {
            spec: "DOM".into(),
            ..Env::default()
        };
        parse_source_with(&sources(&[step]).remove(0), &env)
    }
    fn kinds(p: &ParsedSource) -> Vec<&StatementKind> {
        p.statements.iter().map(|s| &s.kind).collect()
    }

    #[test]
    fn loops_with_an_inline_body_after_a_colon() {
        let p = parse(
            "For each <var>node</var> of <var>nodes</var>: set <var>x</var> to <var>node</var>.",
        );
        let [StatementKind::ForEach {
            vars,
            body: BodyLoc::InlineRest,
            ..
        }, StatementKind::Set { .. }] = kinds(&p)[..]
        else {
            panic!("{:?}", kinds(&p))
        };
        assert_eq!(vars, &["node"]);
        let parent = p.statements[1].parent.as_ref().unwrap();
        assert_eq!(
            (parent.statement_id.as_str(), parent.role),
            (p.statements[0].id.as_str(), BlockRole::Body)
        );
        for anchor in ["list-iterate", "map-iterate"] {
            let p = parse(&format!(
                r##"<a href="https://infra.spec.whatwg.org/#{anchor}">For each</a> <var>k</var> → <var>v</var> of <var>m</var>: set <var>x</var> to <var>v</var>."##
            ));
            let [StatementKind::ForEach { vars, .. }, StatementKind::Set { .. }] = kinds(&p)[..]
            else {
                panic!("{anchor}: {:?}", kinds(&p))
            };
            assert_eq!(vars, &["k", "v"]);
            assert_eq!(p.roles[&0], crate::state::ir::LinkRole::Keyword);
        }
        let p = parse("While <var>x</var> is non-null: set <var>x</var> to <var>y</var>.");
        let [StatementKind::While {
            condition: Some(_),
            body: BodyLoc::InlineRest,
            ..
        }, StatementKind::Set { .. }] = kinds(&p)[..]
        else {
            panic!("{:?}", kinds(&p))
        };
        assert_eq!(
            p.statements[1].parent.as_ref().unwrap().role,
            BlockRole::Body
        );
    }

    #[test]
    fn inline_if_then_return_with_parent() {
        let p = parse("If <var>x</var> is null, then return.");
        let [StatementKind::If {
            condition,
            then: BodyLoc::InlineRest,
        }, StatementKind::Return { value: None }] = kinds(&p)[..]
        else {
            panic!("{:?}", kinds(&p))
        };
        assert_eq!(
            condition,
            &Predicate::Is {
                operand: var("x"),
                test: Test::Null,
                negated: false
            }
        );
        assert_eq!(
            p.statements[1]
                .parent
                .as_ref()
                .map(|q| (q.statement_id.as_str(), q.role)),
            Some((p.statements[0].id.as_str(), BlockRole::Then))
        );
        assert_eq!(p.statements[0].parent, None);
    }

    #[test]
    fn inline_otherwise_is_a_sibling_of_its_if() {
        let p = parse(
            "If <var>x</var> is null, then return <var>a</var>; otherwise, return <var>b</var>.",
        );
        let if_id = p.statements[0].id.clone();
        let other = p
            .statements
            .iter()
            .find(|s| matches!(s.kind, StatementKind::Otherwise { .. }))
            .unwrap();
        assert!(
            matches!(&other.kind, StatementKind::Otherwise { of: Some(of), condition: None, body: BodyLoc::InlineRest } if of == &if_id)
        );
        assert_eq!(other.parent, None);
        let last = p.statements.last().unwrap();
        assert!(matches!(&last.kind, StatementKind::Return { value: Some(v) } if v == &var("b")));
        assert_eq!(last.parent.as_ref().unwrap().role, BlockRole::Otherwise);
    }

    #[test]
    fn loops_with_child_bodies_filters_and_order() {
        let p = parse("For each <var>child</var> of <var>node</var>’s <a href=\"#concept-tree-child\">children</a>:");
        let StatementKind::ForEach {
            vars,
            collection: Expr::Path(_),
            filter: None,
            order: None,
            body: BodyLoc::ChildSteps { .. },
        } = &p.statements[0].kind
        else {
            panic!()
        };
        assert_eq!(vars, &["child"]);
        let p = parse("For each <var>k</var> → <var>v</var> of <var>m</var>:");
        assert!(
            matches!(&p.statements[0].kind, StatementKind::ForEach { vars, .. } if vars == &["k", "v"])
        );
        let p = parse("For each <code>NodeIterator</code> object <var>iterator</var> whose <a href=\"#iterator-root\">root</a>’s <a href=\"#concept-node-document\">node document</a> is <var>node</var>’s <a href=\"#concept-node-document\">node document</a>, in <a href=\"#concept-tree-order\">tree order</a>:");
        assert!(
            matches!(&p.statements[0].kind, StatementKind::ForEach { vars, filter: Some(Predicate::Compare { .. }), order: Some(o), .. } if vars == &["iterator"] && o == "tree order")
        );
        let p = parse("While <var>x</var> is not null:");
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::While {
                condition: Some(Predicate::Is { negated: true, .. }),
                form: LoopForm::While,
                ..
            }
        ));
        assert!(matches!(
            &parse("Repeat:").statements[0].kind,
            StatementKind::While {
                condition: None,
                form: LoopForm::Repeat,
                ..
            }
        ));
    }

    #[test]
    fn exits_waits_parallel_and_asserts() {
        let p = parse("Throw a \"<a href=\"https://webidl.spec.whatwg.org/#notfounderror\"><code>NotFoundError</code></a>\" <a href=\"https://webidl.spec.whatwg.org/#dfn-DOMException\"><code>DOMException</code></a>.");
        let StatementKind::Throw { exception } = &p.statements[0].kind else {
            panic!()
        };
        assert_eq!(
            (
                exception.name.as_deref(),
                exception.link.as_ref().unwrap().anchor.as_str()
            ),
            (Some("NotFoundError"), "dfn-DOMException")
        );
        assert!(
            matches!(&parse("Abort these steps.").statements[0].kind, StatementKind::Abort { text } if text == "these steps")
        );
        assert!(matches!(
            parse("Continue.").statements[0].kind,
            StatementKind::Continue
        ));
        assert!(matches!(
            parse("<a href=\"https://infra.spec.whatwg.org/#iteration-continue\">Continue</a>.")
                .statements[0]
                .kind,
            StatementKind::Continue
        ));
        assert!(matches!(
            parse("Break.").statements[0].kind,
            StatementKind::Break
        ));
        assert!(matches!(
            &parse("Wait until <var>done</var> is true.").statements[0].kind,
            StatementKind::Wait {
                condition: Some(Predicate::Is {
                    test: Test::True,
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            parse("<a href=\"#in-parallel\">In parallel</a>:").statements[0].kind,
            StatementKind::InParallel { .. }
        ));
        assert!(matches!(
            parse("Run the following steps:").statements[0].kind,
            StatementKind::RunSteps { .. }
        ));
        let p = parse("<a href=\"https://infra.spec.whatwg.org/#assert\">Assert</a>: <var>x</var> is non-null.");
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Assert {
                predicate: Predicate::Is {
                    test: Test::Null,
                    negated: true,
                    ..
                }
            }
        ));
        assert_eq!(p.roles[&0], crate::state::ir::LinkRole::Keyword);
        assert_eq!(p.statements.len(), 1);
    }

    #[test]
    fn inline_loop_bodies_conditional_otherwise_and_plain_throw() {
        let p =
            parse("For each <var>n</var> of <var>list</var>, set <var>x</var> to <var>n</var>.");
        let [StatementKind::ForEach {
            body: BodyLoc::InlineRest,
            ..
        }, StatementKind::Set { .. }] = kinds(&p)[..]
        else {
            panic!("{:?}", kinds(&p))
        };
        assert_eq!(
            p.statements[1]
                .parent
                .as_ref()
                .map(|q| (q.statement_id.as_str(), q.role)),
            Some((p.statements[0].id.as_str(), BlockRole::Body))
        );
        let p = parse("Otherwise, if <var>x</var> is null, then return.");
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Otherwise {
                of: None,
                condition: Some(Predicate::Is {
                    test: Test::Null,
                    ..
                }),
                body: BodyLoc::InlineRest,
            }
        ));
        assert_eq!(
            p.statements[1].parent.as_ref().unwrap().role,
            BlockRole::Otherwise
        );
        let p = parse("Throw a <a href=\"https://webidl.spec.whatwg.org/#exceptiondef-typeerror\"><code>TypeError</code></a>.");
        let StatementKind::Throw { exception } = &p.statements[0].kind else {
            panic!()
        };
        assert_eq!(exception.name.as_deref(), Some("TypeError"));
        assert_eq!(p.roles[&0], crate::state::ir::LinkRole::Type);
        assert!(matches!(
            parse("Optionally, return.").statements[0].kind,
            StatementKind::Return { value: None }
        ));
    }

    #[test]
    fn a_sentence_ends_the_inline_block_and_may_start_otherwise() {
        let p = parse(
            "If <var>p</var> is null, then set <var>x</var> to 1. Otherwise, set <var>y</var> to 2.",
        );
        let [StatementKind::If { .. }, StatementKind::Set { .. }, StatementKind::Otherwise { of: Some(of), .. }, StatementKind::Set { .. }] =
            kinds(&p)[..]
        else {
            panic!("{:?}", kinds(&p))
        };
        let if_id = &p.statements[0].id;
        assert_eq!(of, if_id);
        let parents: Vec<_> = p
            .statements
            .iter()
            .map(|s| s.parent.as_ref().map(|q| (q.statement_id.as_str(), q.role)))
            .collect();
        assert_eq!(
            parents,
            [
                None,
                Some((if_id.as_str(), BlockRole::Then)),
                None,
                Some((p.statements[2].id.as_str(), BlockRole::Otherwise)),
            ]
        );
        let p = parse("If <var>p</var> is null, then set <var>x</var> to 1. The user agent may wait, and then set <var>y</var> to 2.");
        let last = p.statements.last().unwrap();
        assert!(
            matches!(
                last,
                Statement {
                    kind: StatementKind::Set { .. },
                    parent: None,
                    ..
                }
            ),
            "{last:?}"
        );
    }

    #[test]
    fn linked_control_keywords_are_control_not_calls() {
        let p = parse_in_dom(
            r##"If <var>selector</var> is failure, then <a href="https://webidl.spec.whatwg.org/#dfn-throw">throw</a> a "<code class="idl"><a href="https://webidl.spec.whatwg.org/#syntaxerror">SyntaxError</a></code>" <code class="idl"><a href="https://webidl.spec.whatwg.org/#idl-DOMException">DOMException</a></code>."##,
        );
        let [StatementKind::If { .. }, StatementKind::Throw { exception }] = kinds(&p)[..] else {
            panic!("{:?}", kinds(&p))
        };
        assert_eq!(
            (
                exception.name.as_deref(),
                exception.link.as_ref().unwrap().anchor.as_str()
            ),
            (Some("SyntaxError"), "idl-DOMException")
        );
        assert_eq!(p.roles[&0], crate::state::ir::LinkRole::Keyword);
        assert!(p.calls.is_empty(), "{:?}", p.calls);

        let p = parse_in_dom(
            r##"If <var>child</var> has a <code>type</code> attribute and its value is not a <a href="https://mimesniff.spec.whatwg.org/#mime-type">MIME type</a>, <a href="https://infra.spec.whatwg.org/#iteration-continue">continue</a> to the next child."##,
        );
        let [StatementKind::If { .. }, StatementKind::Continue] = kinds(&p)[..] else {
            panic!("{:?}", kinds(&p))
        };
        assert!(p.calls.is_empty(), "{:?}", p.calls);

        for step in [
            r##"<a href="https://webidl.spec.whatwg.org/#dfn-throw">Throw</a> an exception."##,
            r##"<a href="https://infra.spec.whatwg.org/#list-iterate">For each</a> live range whose start node is <var>parent</var>, set its start to 0."##,
            r##"<a href="https://infra.spec.whatwg.org/#assert">Assert</a> nothing."##,
            r##"<a href="https://infra.spec.whatwg.org/#iteration-break">Break</a> out of the loop."##,
            r##"<a href="https://html.spec.whatwg.org/multipage/infrastructure.html#in-parallel">In parallel</a> do it."##,
        ] {
            assert!(parse_in_dom(step).calls.is_empty(), "{step}");
        }
    }

    #[test]
    fn loops_over_a_tuple() {
        let p = parse(
            "For each (<var>ancestorToReveal</var>, <var>revealType</var>) of <var>ancestorsToReveal</var>:",
        );
        let [StatementKind::ForEach {
            vars, collection, ..
        }] = kinds(&p)[..]
        else {
            panic!("{:?}", kinds(&p))
        };
        assert_eq!(vars, &["ancestorToReveal", "revealType"]);
        assert_eq!(collection, &var("ancestorsToReveal"));
        let p = parse_in_dom(
            r##"<a href="https://infra.spec.whatwg.org/#list-iterate">For each</a> (<var>a</var>, <var>b</var>) of <var>l</var>:"##,
        );
        let [StatementKind::ForEach { vars, .. }] = kinds(&p)[..] else {
            panic!("{:?}", kinds(&p))
        };
        assert_eq!(vars, &["a", "b"]);
        assert!(p.calls.is_empty(), "{:?}", p.calls);
    }

    #[test]
    fn prefixes_are_skipped_and_lowercase_if_is_not_a_head() {
        assert!(matches!(
            parse("⌛ If <var>x</var> is null, then return.").statements[0].kind,
            StatementKind::If { .. }
        ));
        let p = parse("Set <var>y</var> to 1, if <var>x</var> is null.");
        assert!(p
            .statements
            .iter()
            .all(|s| !matches!(s.kind, StatementKind::If { .. })));
    }
}
