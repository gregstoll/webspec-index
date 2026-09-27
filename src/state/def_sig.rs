//! Accessor, predicate, declared and ecmarkup signatures (§8.1.3).
use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::state::grammar::Encoded;
use crate::state::intro::{Intro, IntroVar};
use crate::state::model::{
    Param, Passing, ReturnBasis, ReturnType, Signature, SignatureForm, SignatureIssue, Template,
    TemplatePiece, TypeBasis, TypeExpr, TypeRef,
};
use crate::state::names::NameResolver;
use crate::state::signature::{type_links, type_phrase, words};
use crate::state::typeexpr::parse_intro_type;

/// Phrases after an accessor's receiver that say the steps compute the value.
const ACCESSOR_MARKERS: [&str; 5] = [
    "obtained by",
    "the result of",
    "returned by",
    "as follows",
    "these steps",
];

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// The accessor, predicate or declared signature of a non-ecmarkup intro
/// with a dfn and at least one variable outside it.
pub(crate) fn definitional_signature(intro: &Intro, names: &NameResolver) -> Option<Signature> {
    if intro.ecmarkup {
        return None;
    }
    let dfn = intro.dfn_span?;
    let outside: Vec<&IntroVar> = intro
        .vars
        .iter()
        .filter(|var| var.span.end <= dfn.start || var.span.start >= dfn.end)
        .collect();
    if outside.is_empty() {
        return None;
    }
    let enc = Encoded::new(&intro.source);
    let intro_cx = IntroCx {
        intro,
        names,
        links: type_links(&intro.source, names),
        dfn_start: enc.to_enc(dfn.start),
        dfn_end: enc.to_enc(dfn.end),
        outside,
        enc,
    };
    intro_cx
        .accessor()
        .or_else(|| intro_cx.predicate())
        .or_else(|| Some(intro_cx.declared()))
}

/// The signature an ecmarkup clause states in the `takes … and returns …` sentence that opens
/// its first paragraph; the sentences after it describe the operation.
pub(crate) fn ecmarkup_signature(intro: &Intro) -> Option<Signature> {
    static TAKES: OnceLock<Regex> = OnceLock::new();
    static ITEM: OnceLock<Regex> = OnceLock::new();
    if !intro.ecmarkup || intro.vars.is_empty() {
        return None;
    }
    let enc = Encoded::new(&intro.source);
    let text = enc.text.trim_end();
    let caps = regex(
        &TAKES,
        r"^The .*? takes (?:no arguments|arguments? (?P<args>.+?)) and returns (?P<ret>.+?)\.(?:\s|$)",
    )
    .captures(text)?;
    let mut params = Vec::new();
    let mut pieces = vec![TemplatePiece::Callee, TemplatePiece::Literal("(".into())];
    if let Some(args) = caps.name("args") {
        let mut pos = args.start();
        loop {
            let item =
                regex(&ITEM, r"^⟦V\d+⟧ \((?P<ty>[^)]*)\)").captures(&text[pos..args.end()])?;
            let var = var_at(intro, &enc, pos)?;
            let ty = item.name("ty").unwrap();
            let type_text = src_text(intro, &enc, pos + ty.start(), pos + ty.end());
            let index = params.len() as u32;
            if index > 0 {
                pieces.push(TemplatePiece::ListSep);
            }
            pieces.push(TemplatePiece::Slot(index));
            params.push(param(
                var,
                TypeExpr::Opaque {
                    text: type_text.clone(),
                },
                type_text,
                TypeBasis::Explicit,
                Passing::Positional { index },
            ));
            pos += item.get(0).unwrap().end();
            if pos == args.end() {
                break;
            }
            pos += [", and ", ", ", " and "]
                .iter()
                .find(|sep| text[pos..args.end()].starts_with(**sep))?
                .len();
        }
    }
    pieces.push(TemplatePiece::Literal(")".into()));
    let ret = caps.name("ret").unwrap();
    Some(Signature {
        algorithm: intro.source.subject.clone(),
        source_id: intro.source.id.clone(),
        form: SignatureForm::Ecmarkup,
        this: None,
        params,
        returns: Some(ReturnType {
            ty: TypeExpr::Opaque {
                text: src_text(intro, &enc, ret.start(), ret.end()),
            },
            basis: ReturnBasis::Ecmarkup,
        }),
        template: Some(Template { pieces }),
        issues: Vec::new(),
    })
}

struct IntroCx<'a> {
    intro: &'a Intro,
    names: &'a NameResolver,
    enc: Encoded,
    links: Vec<(String, TypeRef)>,
    dfn_start: usize,
    dfn_end: usize,
    /// The intro's variables outside the dfn, in text order.
    outside: Vec<&'a IntroVar>,
}

/// A parameter's type, its source text and basis.
type Typed = (TypeExpr, String, TypeBasis);

impl IntroCx<'_> {
    /// `The ⟦DFN⟧ of TYPEPH? ⟦V⟧ is|, … (obtained by|the result of|…)`.
    fn accessor(&self) -> Option<Signature> {
        static RETURNS: OnceLock<Regex> = OnceLock::new();
        let text = &self.enc.text;
        if self.lead().0 != "The " || !text[self.dfn_end..].starts_with(" of ") {
            return None;
        }
        let glue_start = self.dfn_end + " of ".len();
        let var = *self
            .outside
            .iter()
            .find(|var| self.enc.to_enc(var.span.start) >= glue_start)?;
        let (var_start, var_end) = self.var_range(var);
        let glue = words(text, glue_start, var_start);
        let typed = if glue.is_empty() {
            None
        } else {
            let phrase = type_phrase(text, &glue, &self.links, self.names)?;
            if phrase.k != 0 {
                return None;
            }
            Some((
                phrase.ty,
                self.src_text(phrase.core_start, var_start)
                    .trim()
                    .to_string(),
                phrase.basis,
            ))
        };
        let after = &text[var_end..];
        if !(after.starts_with(" is") || after.starts_with(','))
            || !ACCESSOR_MARKERS.iter().any(|marker| after.contains(marker))
        {
            return None;
        }
        let returns = regex(
            &RETURNS,
            r"^ is (?:the|a|an) (.+?) (?:obtained by|returned by)",
        )
        .captures(after)
        .and_then(|caps| parse_intro_type(&caps[1], &self.links, self.names, true))
        .map(|(ty, _)| ReturnType {
            ty,
            basis: ReturnBasis::Intro,
        });
        let mut issues = Vec::new();
        let params = vec![self.param(var, typed, Passing::Receiver, &mut issues)];
        Some(self.signature(
            SignatureForm::Accessor,
            params,
            returns,
            Some(vec![
                TemplatePiece::Slot(0),
                TemplatePiece::Literal("'s".into()),
                TemplatePiece::Callee,
            ]),
            issues,
        ))
    }

    /// `(a|an|the) TYPEPH? ⟦V⟧ (is|are) (said to be |considered )?⟦DFN⟧ … if`,
    /// its `has ⟦DFN⟧ if the following steps return true` variant, and
    /// `Two|Three TYPE, ⟦V⟧ and ⟦V⟧, are (said to be |considered )?⟦DFN⟧ … if`.
    fn predicate(&self) -> Option<Signature> {
        static SUBJECT: OnceLock<Regex> = OnceLock::new();
        static COUNTED: OnceLock<Regex> = OnceLock::new();
        static VAR: OnceLock<Regex> = OnceLock::new();
        static IF: OnceLock<Regex> = OnceLock::new();
        let (lead, lead_start) = self.lead();
        let rest = &self.enc.text[self.dfn_end..];
        let mut issues = Vec::new();
        if let Some(caps) = regex(
            &SUBJECT,
            r"^(?:A|An|The|a|an|the) (?P<ty>(?:[^⟦]|⟦L\d+⟧)*?)(?P<var>⟦V\d+⟧) (?:(?P<has>has )|(?:is|are) (?:said to be |considered )?)$",
        )
        .captures(lead)
        {
            let has = caps.name("has").is_some();
            let condition = if has {
                rest.contains(" if the following steps return true")
            } else {
                regex(&IF, r"\bif\b").is_match(rest)
            };
            if !condition {
                return None;
            }
            let ty = caps.name("ty").unwrap();
            let typed = self.stated_type(lead_start + ty.start(), lead_start + ty.end())?;
            let var = self.var_at(lead_start + caps.name("var").unwrap().start())?;
            let params = vec![self.param(var, typed, Passing::Receiver, &mut issues)];
            let verb = if has { "has" } else { "is" };
            return Some(self.signature(
                SignatureForm::Predicate,
                params,
                None,
                Some(vec![
                    TemplatePiece::Slot(0),
                    TemplatePiece::Literal(verb.into()),
                    TemplatePiece::Callee,
                ]),
                issues,
            ));
        }
        let caps = regex(
            &COUNTED,
            r"^(?P<count>Two|Three|two|three) (?P<ty>(?:[^⟦,]|⟦L\d+⟧)+?), (?P<vars>⟦V\d+⟧(?:, ⟦V\d+⟧)*,? and ⟦V\d+⟧),? are (?:said to be |considered )?$",
        )
        .captures(lead)?;
        if !regex(&IF, r"\bif\b").is_match(rest) {
            return None;
        }
        let count = match &caps["count"] {
            "Two" | "two" => 2,
            _ => 3,
        };
        let vars_at = lead_start + caps.name("vars").unwrap().start();
        let vars: Vec<&IntroVar> = regex(&VAR, r"⟦V\d+⟧")
            .find_iter(&caps["vars"])
            .map(|found| self.var_at(vars_at + found.start()))
            .collect::<Option<_>>()?;
        if vars.len() != count {
            return None;
        }
        let ty = caps.name("ty").unwrap();
        let typed = self.stated_type(lead_start + ty.start(), lead_start + ty.end())?;
        let mut pieces = Vec::new();
        let params = vars
            .iter()
            .enumerate()
            .map(|(index, var)| {
                if index > 0 {
                    pieces.push(TemplatePiece::ListSep);
                }
                pieces.push(TemplatePiece::Slot(index as u32));
                self.param(var, typed.clone(), Passing::Receiver, &mut issues)
            })
            .collect();
        pieces.push(TemplatePiece::Literal("are".into()));
        pieces.push(TemplatePiece::Callee);
        Some(self.signature(SignatureForm::Predicate, params, None, Some(pieces), issues))
    }

    /// Every distinct variable outside the dfn, typed by the type phrase
    /// ending the words before it.
    fn declared(&self) -> Signature {
        let text = &self.enc.text;
        let mut issues = Vec::new();
        let mut params = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = 0;
        for var in &self.outside {
            let (var_start, var_end) = self.var_range(var);
            if var_start >= self.dfn_end {
                cursor = cursor.max(self.dfn_end);
            }
            let glue = words(text, cursor, var_start);
            cursor = var_end;
            if !seen.insert(var.name.as_str()) {
                continue;
            }
            let typed = type_phrase(text, &glue, &self.links, self.names).map(|phrase| {
                (
                    phrase.ty,
                    self.src_text(phrase.core_start, var_start)
                        .trim()
                        .to_string(),
                    phrase.basis,
                )
            });
            let index = params.len() as u32;
            params.push(self.param(var, typed, Passing::Positional { index }, &mut issues));
        }
        self.signature(SignatureForm::Declared, params, None, None, issues)
    }

    /// The text from the sentence start to the dfn, and its encoded start.
    fn lead(&self) -> (&str, usize) {
        let before = &self.enc.text[..self.dfn_start];
        let start = before.rfind(". ").map_or(0, |at| at + 2);
        (&before[start..], start)
    }

    /// The type stated by the encoded range, `None` inside when it is empty;
    /// `None` when the range is not a type phrase.
    fn stated_type(&self, start: usize, end: usize) -> Option<Option<Typed>> {
        let core = self.enc.text[start..end].trim();
        if core.is_empty() {
            return Some(None);
        }
        let (ty, basis) = parse_intro_type(core, &self.links, self.names, true)?;
        Some(Some((
            ty,
            self.src_text(start, end).trim().to_string(),
            basis,
        )))
    }

    fn var_range(&self, var: &IntroVar) -> (usize, usize) {
        (
            self.enc.to_enc(var.span.start),
            self.enc.to_enc(var.span.end),
        )
    }

    fn var_at(&self, enc: usize) -> Option<&IntroVar> {
        var_at(self.intro, &self.enc, enc)
    }

    fn src_text(&self, start: usize, end: usize) -> String {
        src_text(self.intro, &self.enc, start, end)
    }

    fn param(
        &self,
        var: &IntroVar,
        typed: Option<Typed>,
        passing: Passing,
        issues: &mut Vec<SignatureIssue>,
    ) -> Param {
        let (ty, type_text, basis) =
            typed.unwrap_or((TypeExpr::Unknown, String::new(), TypeBasis::Unknown));
        match &ty {
            TypeExpr::Unknown => issues.push(SignatureIssue::UntypedParam(var.name.clone())),
            TypeExpr::Opaque { .. } => issues.push(SignatureIssue::OpaqueType(var.name.clone())),
            _ => {}
        }
        param(var, ty, type_text, basis, passing)
    }

    fn signature(
        &self,
        form: SignatureForm,
        params: Vec<Param>,
        returns: Option<ReturnType>,
        template: Option<Vec<TemplatePiece>>,
        issues: Vec<SignatureIssue>,
    ) -> Signature {
        Signature {
            algorithm: self.intro.source.subject.clone(),
            source_id: self.intro.source.id.clone(),
            form,
            this: None,
            params,
            returns,
            template: template.map(|pieces| Template { pieces }),
            issues,
        }
    }
}

/// The intro variable whose placeholder starts at encoded position `enc`.
fn var_at<'a>(intro: &'a Intro, enc: &Encoded, at: usize) -> Option<&'a IntroVar> {
    intro
        .vars
        .iter()
        .find(|var| enc.to_enc(var.span.start) == at)
}

/// Canonical source text of an encoded range.
fn src_text(intro: &Intro, enc: &Encoded, start: usize, end: usize) -> String {
    let span = enc.span(start, end);
    intro.source.text[span.start..span.end].to_string()
}

fn param(
    var: &IntroVar,
    ty: TypeExpr,
    type_text: String,
    type_basis: TypeBasis,
    passing: Passing,
) -> Param {
    Param {
        name: var.name.clone(),
        anchor: var.anchor.clone(),
        ty,
        type_text,
        type_basis,
        optional: false,
        default: None,
        passing,
        span: var.span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::steps::extract_step_structure_from_document;
    use crate::state::intro::{algorithm_intros, IdIndex};
    use crate::state::model::{Passing, ReturnBasis, SignatureForm, TemplatePiece as P};
    use crate::state::testing::*;

    fn def_sigs(html: &str, spec: &str) -> Vec<Signature> {
        let document = scraper::Html::parse_document(html);
        let base = base_url(spec);
        let structure = extract_step_structure_from_document(&document, spec, base, "hash:t");
        let state = extract_html(html, spec);
        let names = crate::state::names::NameResolver::new(
            spec,
            &state.model,
            &crate::state::declare::concept_dfns(&document),
        );
        let index = IdIndex::new(&document);
        algorithm_intros(&document, &index, spec, base, "hash:t", &structure)
            .iter()
            .filter_map(|i| {
                if i.ecmarkup {
                    ecmarkup_signature(i)
                } else {
                    definitional_signature(i, &names)
                }
            })
            .collect()
    }
    fn find<'a>(all: &'a [Signature], anchor: &str) -> &'a Signature {
        all.iter().find(|s| s.algorithm.anchor == anchor).unwrap()
    }

    #[test]
    fn g5_accessor_with_stated_return_type() {
        let all = def_sigs(FALLBACK_HTML, "HTML");
        let s = find(&all, "fallback-base-url");
        assert_eq!(s.form, SignatureForm::Accessor);
        assert_eq!(
            (
                s.params[0].name.as_str(),
                ty(&s.params[0].ty),
                s.params[0].passing.clone()
            ),
            ("document", "idl:Document".to_string(), Passing::Receiver)
        );
        let r = s.returns.as_ref().unwrap();
        assert_eq!(
            (ty(&r.ty), r.basis),
            ("URL#concept-url".to_string(), ReturnBasis::Intro)
        );
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![P::Slot(0), P::Literal("'s".into()), P::Callee]
        );
    }

    #[test]
    fn two_subject_predicate() {
        let all = def_sigs(FALLBACK_HTML, "HTML");
        let s = find(&all, "same-origin");
        assert_eq!(s.form, SignatureForm::Predicate);
        assert_eq!(
            s.params
                .iter()
                .map(|p| (p.name.as_str(), ty(&p.ty)))
                .collect::<Vec<_>>(),
            [
                ("A", "HTML#concept-origin".to_string()),
                ("B", "HTML#concept-origin".to_string())
            ]
        );
        assert!(s.params.iter().all(|p| p.passing == Passing::Receiver));
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![
                P::Slot(0),
                P::ListSep,
                P::Slot(1),
                P::Literal("are".into()),
                P::Callee
            ]
        );
    }

    #[test]
    fn one_subject_predicates_and_untyped_accessors() {
        let html = r##"<pre><code class="idl">interface <dfn id="node">Node</dfn> {};</code></pre>
<div data-algorithm=""><p>A <code><a href="#node">Node</a></code> <var>node</var> is said to be <dfn id="connected">connected</dfn> if the following steps return true:</p><ol><li><p>Return true.</p></li></ol></div>
<div data-algorithm=""><p>A node <var>node</var> has <dfn id="has-kids">kids</dfn> if the following steps return true:</p><ol><li><p>Return true.</p></li></ol></div>
<div data-algorithm=""><p>The <dfn id="depth">depth</dfn> of <var>node</var>, which is a number, is the result of these steps:</p><ol><li><p>Return 0.</p></li></ol></div>
<div data-algorithm=""><p>Three <a href="#node">nodes</a>, <var>a</var>, <var>b</var>, and <var>c</var>, are <dfn id="aligned">aligned</dfn> if these steps return true:</p><ol><li><p>Return true.</p></li></ol></div>"##;
        let all = def_sigs(html, "HTML");
        let connected = find(&all, "connected");
        assert_eq!(connected.form, SignatureForm::Predicate);
        assert_eq!(ty(&connected.params[0].ty), "idl:Node");
        assert_eq!(connected.returns, None);
        assert_eq!(
            connected.template.as_ref().unwrap().pieces,
            vec![P::Slot(0), P::Literal("is".into()), P::Callee]
        );
        let kids = find(&all, "has-kids");
        assert_eq!(kids.form, SignatureForm::Predicate);
        assert_eq!(ty(&kids.params[0].ty), "idl:Node");
        assert_eq!(
            kids.template.as_ref().unwrap().pieces,
            vec![P::Slot(0), P::Literal("has".into()), P::Callee]
        );
        let depth = find(&all, "depth");
        assert_eq!(depth.form, SignatureForm::Accessor);
        assert_eq!(
            (ty(&depth.params[0].ty), depth.params[0].passing.clone()),
            ("?".to_string(), Passing::Receiver)
        );
        assert_eq!(depth.returns, None);
        assert_eq!(
            depth.issues,
            vec![crate::state::model::SignatureIssue::UntypedParam(
                "node".into()
            )]
        );
        let aligned = find(&all, "aligned");
        assert_eq!(
            aligned
                .params
                .iter()
                .map(|p| (p.name.as_str(), ty(&p.ty)))
                .collect::<Vec<_>>(),
            [
                ("a", "HTML#node".to_string()),
                ("b", "HTML#node".to_string()),
                ("c", "HTML#node".to_string())
            ]
        );
        assert_eq!(
            aligned.template.as_ref().unwrap().pieces,
            vec![
                P::Slot(0),
                P::ListSep,
                P::Slot(1),
                P::ListSep,
                P::Slot(2),
                P::Literal("are".into()),
                P::Callee
            ]
        );
    }

    #[test]
    fn declared_parameters_are_distinct_variables_in_text_order() {
        let html = r##"<div data-algorithm=""><p>For a string <var>s</var>, the <dfn id="y-steps">y steps</dfn> given a boolean <var>b</var> read <var>s</var> twice:</p><ol><li><p>Return.</p></li></ol></div>"##;
        let all = def_sigs(html, "HTML");
        let s = find(&all, "y-steps");
        assert_eq!(s.form, SignatureForm::Declared);
        assert_eq!(
            s.params
                .iter()
                .map(|p| (p.name.as_str(), ty(&p.ty), p.passing.clone()))
                .collect::<Vec<_>>(),
            [
                ("s", "string".to_string(), Passing::Positional { index: 0 }),
                ("b", "boolean".to_string(), Passing::Positional { index: 1 })
            ]
        );
    }

    #[test]
    fn declared_intros_have_parameters_but_no_template() {
        let html = r##"<pre><code class="idl">partial interface <dfn data-lt="" id="document">Document</dfn> {};</code></pre>
<div data-algorithm=""><p>The <dfn id="x-steps">x steps</dfn> for a <code><a href="#document">Document</a></code> <var>document</var> are as follows:</p><ol><li><p>Return.</p></li></ol></div>"##;
        let all = def_sigs(html, "HTML");
        let s = find(&all, "x-steps");
        assert_eq!(
            (s.form.clone(), s.template.clone()),
            (SignatureForm::Declared, None)
        );
        assert_eq!(ty(&s.params[0].ty), "idl:Document");
        assert!(def_sigs(AUTODIR_HTML, "HTML")
            .iter()
            .all(|s| s.algorithm.anchor != "rules-to-parse-a-date-string"));
    }

    #[test]
    fn ecmarkup_takes_arguments_and_returns() {
        let all = def_sigs(ECMA_HTML, "ECMA-262");
        let s = find(&all, "sec-stringindexof");
        assert_eq!(s.form, SignatureForm::Ecmarkup);
        assert_eq!(
            s.params
                .iter()
                .map(|p| (p.name.as_str(), ty(&p.ty)))
                .collect::<Vec<_>>(),
            [
                ("string", "opaque(a String)".to_string()),
                ("searchValue", "opaque(a String)".to_string()),
                ("fromIndex", "opaque(a non-negative integer)".to_string())
            ]
        );
        let r = s.returns.as_ref().unwrap();
        assert_eq!(r.basis, ReturnBasis::Ecmarkup);
        assert_eq!(ty(&r.ty), "opaque(a non-negative integer or `not-found`)");
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                P::Literal("(".into()),
                P::Slot(0),
                P::ListSep,
                P::Slot(1),
                P::ListSep,
                P::Slot(2),
                P::Literal(")".into())
            ]
        );
        let caller = find(&all, "sec-caller");
        assert_eq!(caller.params.len(), 1);
        assert_eq!(
            ty(&caller.returns.as_ref().unwrap().ty),
            "opaque(an integer)"
        );
    }
}
