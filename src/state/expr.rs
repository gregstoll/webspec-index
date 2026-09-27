//! `VALUE` grammar: expression atoms (§7.3), extended with calls and
//! conditionals by later productions.
use crate::state::grammar::{literal, Parser, Placeholder};
use crate::state::ir::{Expr, LinkRole};
use crate::state::model::Literal;

const QUOTES: [(&str, &str); 2] = [("\"", "\""), ("\u{201C}", "\u{201D}")];

impl Parser<'_> {
    /// `VALUE` → `Expr`. Every link inside a parsed (non-`Opaque`) atom gets
    /// the `Value` role.
    pub(crate) fn expr_at(&mut self, start: usize, end: usize) -> Expr {
        let mut values = Vec::new();
        let expr = self.value_expr(start, end, &mut values);
        for link in values {
            self.set_role(link, LinkRole::Value);
        }
        expr
    }

    /// `VALUE` → `Expr` without recording roles; the link indices of parsed
    /// atoms go to `values`. `PATH` subscripts use it directly.
    pub(crate) fn value_expr(&self, start: usize, end: usize, values: &mut Vec<usize>) -> Expr {
        let text = &self.enc.text[start..end];
        let start = start + (text.len() - text.trim_start().len());
        let end = end - (text.len() - text.trim_end().len());
        let start = start.min(end);
        let expr = self.atom(start, end, values);
        if !matches!(expr, Expr::Opaque { .. } | Expr::List(_)) {
            values.extend(self.enc.links_in(start, end));
        }
        expr
    }

    fn atom(&self, start: usize, end: usize, values: &mut Vec<usize>) -> Expr {
        let text = &self.enc.text[start..end];
        match self.enc.placeholder(start) {
            Some((Placeholder::Var(var), after)) if after == end => {
                return Expr::Var(self.enc.vars[var].clone());
            }
            Some((Placeholder::Link(link), after)) if after == end && self.is_this_link(link) => {
                return Expr::This;
            }
            _ => {}
        }
        if text == "this" {
            return Expr::This;
        }
        // A quoted code token is an enum value, not the string `literal`
        // reads it as.
        if QUOTES
            .iter()
            .any(|(open, close)| text.starts_with(open) && text.ends_with(close))
        {
            if let Some(value) = self.enum_value(start, end) {
                return value;
            }
        }
        if let Some(lit) = self.literal_at(start, end) {
            return Expr::Literal(lit);
        }
        if let Some(items) = self.list(start, end) {
            return Expr::List(
                items
                    .into_iter()
                    .map(|(start, end)| self.value_expr(start, end, values))
                    .collect(),
            );
        }
        if let Some(after) = self.keyword(start, &["a new ", "an new "]) {
            return Expr::New {
                ty: self.new_type(after),
                init: self.inits.get(&start).cloned(),
            };
        }
        if let Some(path) = self.path(start, false).filter(|path| path.end == end) {
            return Expr::Path(path.path);
        }
        if let Some(value) = self.enum_value(start, end) {
            return value;
        }
        Expr::Opaque {
            text: self.src_text(start, end),
        }
    }

    /// `LIT`: `literal`, `failure`, `the empty string`, a number with a
    /// U+2212 minus, or a code token spelling a literal.
    fn literal_at(&self, start: usize, end: usize) -> Option<Literal> {
        let text = &self.enc.text[start..end];
        match text {
            "failure" => return Some(Literal::Failure),
            "the empty string" => return Some(Literal::String(String::new())),
            _ => {}
        }
        if let Some(digits) = text.strip_prefix('\u{2212}') {
            if let Some(Literal::Number(n)) = literal(digits).filter(|_| !digits.starts_with('-')) {
                return Some(Literal::Number(format!("-{n}")));
            }
        }
        if let Some((inner, after)) = self.code(start) {
            if after == end {
                return literal(&inner);
            }
        }
        // Placeholder text is never a literal's content.
        if text.contains('\u{27E6}') {
            return None;
        }
        literal(text)
    }

    /// `LIST`: `« »` or `« A, B »` spanning `start..end`; the element ranges.
    fn list(&self, start: usize, end: usize) -> Option<Vec<(usize, usize)>> {
        let inner_start = self.keyword(start, &["\u{AB}"])?;
        let inner_end = end.checked_sub('\u{BB}'.len_utf8())?;
        if inner_end < inner_start || !self.lit(inner_end, "\u{BB}") {
            return None;
        }
        let mut items = Vec::new();
        let mut item_start = inner_start;
        let mut depth = 0usize;
        let mut pos = inner_start;
        while pos < inner_end {
            if let Some((_, after)) = self.enc.placeholder(pos) {
                pos = after;
                continue;
            }
            let ch = self.enc.text[pos..].chars().next().expect("in bounds");
            if !self.enc.is_protected(pos) {
                match ch {
                    '\u{AB}' | '(' => depth += 1,
                    // The opening `«` closes before the end: not one list.
                    '\u{BB}' | ')' => depth = depth.checked_sub(1)?,
                    ',' if depth == 0 && self.lit(pos, ", ") => {
                        items.push((item_start, pos));
                        item_start = pos + ", ".len();
                    }
                    _ => {}
                }
            }
            pos += ch.len_utf8();
        }
        if depth != 0 {
            return None;
        }
        if !self.enc.text[item_start..inner_end].trim().is_empty() || !items.is_empty() {
            items.push((item_start, inner_end));
        }
        Some(items)
    }

    /// `ENUM`: a code token or a link with backtick-delimited text, either
    /// optionally quoted.
    fn enum_value(&self, start: usize, end: usize) -> Option<Expr> {
        let text = &self.enc.text[start..end];
        let (start, end) = QUOTES
            .iter()
            .find(|(open, close)| {
                text.len() >= open.len() + close.len()
                    && text.starts_with(open)
                    && text.ends_with(close)
            })
            .map_or((start, end), |(open, close)| {
                (start + open.len(), end - close.len())
            });
        if let Some((Placeholder::Link(link), after)) = self.enc.placeholder(start) {
            let link = &self.source.links[link];
            let text = link.visible_text.strip_prefix('`')?.strip_suffix('`')?;
            return (after == end).then(|| Expr::EnumValue {
                text: text.to_string(),
                target: link.target.clone(),
            });
        }
        let (inner, after) = self.code(start)?;
        (after == end && self.enc.is_protected(start + 1)).then_some(Expr::EnumValue {
            text: inner,
            target: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::state::grammar::{Encoded, Env, Parser};
    use crate::state::ir::{tests_support::sources, Expr, Hop, Path, Root};
    use crate::state::model::Literal;
    use crate::state::testing::var;

    /// Parse the whole of `html` (one step) as an expression.
    fn expr(html: &str) -> Expr {
        let src = sources(&[html]).remove(0);
        let enc = Encoded::new(&src);
        let env = Env::default();
        let mut p = Parser::new(&enc, &src, &env);
        let end = enc.text.trim_end_matches('.').len();
        p.expr_at(0, end)
    }

    #[test]
    fn atoms() {
        assert_eq!(expr("<var>x</var>"), var("x"));
        assert_eq!(
            expr("<a href=\"https://webidl.spec.whatwg.org/#this\">this</a>"),
            Expr::This
        );
        assert_eq!(
            expr("the empty string"),
            Expr::Literal(Literal::String(String::new()))
        );
        assert_eq!(expr("failure"), Expr::Literal(Literal::Failure));
        assert_eq!(expr("−1"), Expr::Literal(Literal::Number("-1".into())));
        assert_eq!(
            expr("<code>true</code>"),
            Expr::Literal(Literal::Bool(true))
        );
    }

    #[test]
    fn lists_and_enum_values() {
        assert_eq!(expr("« »"), Expr::List(vec![]));
        assert_eq!(expr("« <var>node</var> »"), Expr::List(vec![var("node")]));
        assert_eq!(
            expr("« <var>a</var>, « <var>b</var>, <var>c</var> » »"),
            Expr::List(vec![var("a"), Expr::List(vec![var("b"), var("c")])])
        );
        assert_eq!(
            expr("\"<code>replace</code>\""),
            Expr::EnumValue {
                text: "replace".into(),
                target: None
            }
        );
        let Expr::EnumValue { text, target } =
            expr("\"<code><a href=\"#nhb-auto\">auto</a></code>\"")
        else {
            panic!()
        };
        assert_eq!(
            (text.as_str(), target.unwrap().anchor.as_str()),
            ("auto", "nhb-auto")
        );
    }

    #[test]
    fn paths_and_opaque() {
        let Expr::Path(Path {
            root: Root::Var(root),
            hops,
            ..
        }) = expr("<var>document</var>'s <a href=\"#url\">URL</a>")
        else {
            panic!()
        };
        assert_eq!(root, "document");
        assert!(matches!(&hops[0], Hop::Field { visible_text, .. } if visible_text == "URL"));
        assert!(matches!(
            expr("the number of seconds since midnight"),
            Expr::Opaque { .. }
        ));
    }

    #[test]
    fn value_role_only_for_parsed_expressions() {
        let src = sources(&["<var>d</var>'s <a href=\"#url\">URL</a>"]).remove(0);
        let enc = Encoded::new(&src);
        let env = Env::default();
        let mut p = Parser::new(&enc, &src, &env);
        let _ = p.expr_at(0, enc.text.len());
        assert_eq!(
            p.out.roles.get(&0),
            Some(&crate::state::ir::LinkRole::Value)
        );

        let src = sources(&["the number of <a href=\"#url\">URLs</a> seen"]).remove(0);
        let enc = Encoded::new(&src);
        let mut p = Parser::new(&enc, &src, &env);
        assert!(matches!(p.expr_at(0, enc.text.len()), Expr::Opaque { .. }));
        assert!(p.out.roles.is_empty());
    }

    #[test]
    fn a_list_must_span_the_whole_range() {
        assert!(matches!(
            expr("« <var>a</var> » and « <var>b</var> »"),
            Expr::Opaque { .. }
        ));
    }
}
