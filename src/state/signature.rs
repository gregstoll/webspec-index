//! `To`, `WhenStepsSay` and `GivenList` signatures and their call templates (§8.1).
use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::parse::idl_defs::normalize_owner;
use crate::state::def_sig::{definitional_signature, ecmarkup_signature};
use crate::state::grammar::{Encoded, Env, Parser};
use crate::state::idl_sig::{idl_signature, IdlMembers};
use crate::state::intro::Intro;
use crate::state::ir::StatementSource;
use crate::state::model::{
    Param, Passing, ReturnBasis, ReturnType, Signature, SignatureForm, SignatureIssue, Template,
    TemplatePiece, TypeBasis, TypeExpr, TypeRef,
};
use crate::state::names::NameResolver;
use crate::state::typeexpr::parse_intro_type;

/// Words stripped before a type phrase's core.
const ARTICLES: [&str; 6] = ["a", "an", "the", "optional", "optionally", "particular"];
/// Words a type phrase without an article may start with.
const TYPE_WORDS: [&str; 13] = [
    "null",
    "boolean",
    "string",
    "number",
    "integer",
    "byte sequence",
    "scalar value string",
    "list",
    "ordered set",
    "ordered map",
    "map",
    "tuple",
    "struct",
];
const TERMINATORS: [&str; 8] = [
    ":",
    ", run these steps:",
    ", run these steps.",
    ", run the following steps:",
    ", perform the following steps:",
    ", perform the following steps.",
    ", the user agent must run these steps:",
    "",
];

/// The signature an intro of form `To`, `WhenStepsSay` or `GivenList` states;
/// `None` for any other intro.
pub(crate) fn to_signature(intro: &Intro, names: &NameResolver) -> Option<Signature> {
    static GIVEN_LIST: OnceLock<Regex> = OnceLock::new();
    let dfn = intro.dfn_span?;
    let enc = Encoded::new(&intro.source);
    let (dfn_start, dfn_end) = (enc.to_enc(dfn.start), enc.to_enc(dfn.end));
    let before = &enc.text[..dfn_start];
    let lead_start = before.rfind(". ").map_or(0, |at| at + 2);
    let lead = &before[lead_start..];
    let (form, head_start) = if lead.starts_with("To ") {
        (SignatureForm::To, lead_start + "To ".len())
    } else if lead.starts_with("When the steps below say to ") {
        (
            SignatureForm::WhenStepsSay,
            lead_start + "When the steps below say to ".len(),
        )
    } else if lead == "The "
        && GIVEN_LIST
            .get_or_init(|| Regex::new(r"^(?: [^.⟦]+?){0,8}, given ").unwrap())
            .is_match(&enc.text[dfn_end..])
    {
        (SignatureForm::GivenList, dfn_start)
    } else {
        return None;
    };

    let env = Env::default();
    let mut parser = Parser::new(&enc, &intro.source, &env);
    let links = type_links(&intro.source, names);

    let mut pieces = Vec::new();
    let head = parser.src_text(head_start, dfn_start);
    if !head.trim().is_empty() {
        pieces.push(TemplatePiece::Head(head.trim().to_string()));
    }
    pieces.push(TemplatePiece::Callee);

    let mut params: Vec<Param> = Vec::new();
    let mut issues = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor = dfn_end;
    let mut positional = 0u32;
    let mut named_group = false;
    let mut sticky_optional = false;
    let mut tail_consumed = false;
    // Parameters a COUNT type phrase still applies to, with that type.
    let mut counted: Option<(usize, TypeExpr, TypeBasis, String)> = None;
    for var in &intro.vars {
        let var_start = enc.to_enc(var.span.start);
        let var_end = enc.to_enc(var.span.end);
        if var_start < cursor {
            continue;
        }
        if enc.text[cursor..var_start].contains(". ") {
            break;
        }
        let mut words = words(&enc.text, cursor, var_start);
        if enc.text[cursor..var_start]
            .trim_start()
            .starts_with("which ")
        {
            let comma = words.iter().position(|&(s, e)| &enc.text[s..e] == ",");
            if let Some(comma) = comma {
                words.drain(..=comma);
            }
        }
        let texts: Vec<String> = words
            .iter()
            .map(|&(start, end)| parser.src_text(start, end))
            .collect();
        let has = |word: &str| texts.iter().any(|t| t == word);
        sticky_optional |= has("optionally");
        let mut optional = sticky_optional || has("optional");

        let phrase = type_phrase(&enc.text, &words, &links, names);
        let (mut ty, mut type_basis, mut type_text, literal_words) = match &phrase {
            Some(found) => (
                found.ty.clone(),
                found.basis,
                parser
                    .src_text(found.core_start, var_start)
                    .trim()
                    .to_string(),
                &texts[..found.k],
            ),
            None => match described_type(&texts) {
                Some((k, core)) => {
                    let text = parser
                        .src_text(words[core].0, var_start)
                        .trim()
                        .trim_end_matches(',')
                        .to_string();
                    (
                        TypeExpr::Opaque { text: text.clone() },
                        TypeBasis::Explicit,
                        text,
                        &texts[..k],
                    )
                }
                None => (
                    TypeExpr::Unknown,
                    TypeBasis::Unknown,
                    String::new(),
                    &texts[..],
                ),
            },
        };
        let mut literal = normalize_literal(literal_words);
        match &phrase {
            Some(found) => {
                counted = count(&enc.text[found.core_start..var_start])
                    .map(|(n, _)| (n - 1, ty.clone(), type_basis, type_text.clone()));
            }
            None if literal.is_empty() => {
                if let Some((left @ 1.., carried_ty, carried_basis, carried_text)) = &mut counted {
                    *left -= 1;
                    (ty, type_basis, type_text) =
                        (carried_ty.clone(), *carried_basis, carried_text.clone());
                } else {
                    counted = None;
                }
            }
            None => counted = None,
        }

        let mut end = var_end;
        let mut default = None;
        if parser.lit(var_end, " (default ") {
            let inside = var_end + " (default ".len();
            match closing_paren(&enc, inside) {
                Some(close) => {
                    default = Some(parser.expr_at(inside, close));
                    end = close + 1;
                }
                None => {
                    let rest = parser.src_text(var_end, enc.text.len());
                    issues.push(SignatureIssue::UnparsedIntroTail(rest.trim().to_string()));
                    tail_consumed = true;
                }
            }
        }
        optional |= default.is_some();

        if !seen.insert(var.name.clone()) {
            issues.push(SignatureIssue::DuplicateParam(var.name.clone()));
        }
        match &ty {
            TypeExpr::Unknown => issues.push(SignatureIssue::UntypedParam(var.name.clone())),
            TypeExpr::Opaque { .. } => issues.push(SignatureIssue::OpaqueType(var.name.clone())),
            _ => {}
        }

        if form == SignatureForm::GivenList && params.is_empty() {
            literal = "given".to_string();
        }
        let passing = if var.param_dfn && form != SignatureForm::GivenList {
            if !named_group {
                pieces.push(TemplatePiece::NamedGroup(literal));
                named_group = true;
            }
            Passing::Named
        } else {
            if !literal.is_empty() {
                pieces.push(TemplatePiece::Literal(literal));
            } else if positional > 0 {
                pieces.push(TemplatePiece::ListSep);
            }
            pieces.push(TemplatePiece::Slot(positional));
            positional += 1;
            Passing::Positional {
                index: positional - 1,
            }
        };

        params.push(Param {
            name: var.name.clone(),
            anchor: var.anchor.clone(),
            ty,
            type_text,
            type_basis,
            optional,
            default,
            passing,
            span: var.span,
        });
        cursor = end;
        if tail_consumed {
            break;
        }
    }

    let mut returns = None;
    if !tail_consumed {
        match terminator(&enc.text[cursor..], &links, names) {
            Ok(ty) => {
                returns = ty.map(|ty| ReturnType {
                    ty,
                    basis: ReturnBasis::Intro,
                })
            }
            Err(()) => {
                let rest = parser.src_text(cursor, enc.text.len());
                issues.push(SignatureIssue::UnparsedIntroTail(rest.trim().to_string()));
            }
        }
    }

    if let Some(owner) = &intro.dfn_for {
        apply_dfn_for(&mut params, &mut issues, owner, names);
    }

    Some(Signature {
        algorithm: intro.source.subject.clone(),
        source_id: intro.source.id.clone(),
        form,
        this: None,
        params,
        returns,
        template: Some(Template { pieces }),
        issues,
    })
}

/// One signature per intro that states one, first match wins: IDL, `To`-like,
/// ecmarkup (ecmarkup intros only), definitional. The first intro of an anchor
/// wins.
pub(crate) fn extract_signatures(
    intros: &[Intro],
    names: &NameResolver,
    members: &IdlMembers,
) -> Vec<Signature> {
    let mut seen = HashSet::new();
    intros
        .iter()
        .filter_map(|intro| {
            if seen.contains(intro.anchor.as_str()) {
                return None;
            }
            let signature = idl_signature(intro, members)
                .or_else(|| to_signature(intro, names))
                .or_else(|| intro.ecmarkup.then(|| ecmarkup_signature(intro)).flatten())
                .or_else(|| definitional_signature(intro, names))?;
            seen.insert(intro.anchor.as_str());
            Some(signature)
        })
        .collect()
}

/// Each link's visible text and the type it denotes in a type position.
pub(crate) fn type_links(source: &StatementSource, names: &NameResolver) -> Vec<(String, TypeRef)> {
    source
        .links
        .iter()
        .map(|link| (link.visible_text.clone(), names.link_type(link)))
        .collect()
}

/// The type phrase ending a glue: the words from `k` on, `core_start` after
/// its article words.
pub(crate) struct TypePhrase {
    pub k: usize,
    pub core_start: usize,
    pub ty: TypeExpr,
    pub basis: TypeBasis,
}

/// The longest suffix of `words` that is an intro type phrase.
pub(crate) fn type_phrase(
    text: &str,
    words: &[(usize, usize)],
    links: &[(String, TypeRef)],
    names: &NameResolver,
) -> Option<TypePhrase> {
    let end = words.last()?.1;
    if matches!(&text[words.last()?.0..end], "a" | "an" | "the") {
        return None;
    }
    (0..words.len()).find_map(|k| {
        let mut j = k;
        while j < words.len() && ARTICLES.contains(&&text[words[j].0..words[j].1]) {
            j += 1;
        }
        let core_start = words.get(j)?.0;
        let core = &text[core_start..end];
        let stripped = j > k;
        if !stripped && !starts_typed(core) {
            return None;
        }
        let (ty, basis) = parse_intro_type(core, links, names, stripped)?;
        Some(TypePhrase {
            k,
            core_start,
            ty,
            basis,
        })
    })
}

/// A glue with no type phrase whose words from an article or COUNT word on
/// describe the type in prose ("and a series of steps", "given a struct
/// consisting of a width and a height, or null,", "four policy
/// container-or-nulls"): the index of that word, where the literal ends, and
/// of the description's first word.
fn described_type(texts: &[String]) -> Option<(usize, usize)> {
    let k = texts.iter().position(|w| {
        matches!(
            w.to_lowercase().as_str(),
            "a" | "an" | "the" | "two" | "three" | "four"
        )
    })?;
    let core = if matches!(texts[k].to_lowercase().as_str(), "a" | "an" | "the") {
        k + 1
    } else {
        k
    };
    texts[core..]
        .iter()
        .any(|w| w != "," && !ARTICLES.contains(&w.as_str()))
        .then_some((k, core))
}

/// A core that names a type on its own: a link, a quote, `null`, a primitive
/// or an Infra word.
fn starts_typed(core: &str) -> bool {
    if let Some(rest) = count(core).map(|(_, rest)| rest) {
        return starts_typed(rest);
    }
    core.starts_with("⟦L")
        || core.starts_with(['"', '\u{201C}'])
        || TYPE_WORDS.iter().any(|word| {
            core.strip_prefix(word)
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric()))
        })
}

/// A leading COUNT word (§8.1.4): how many parameters the type applies to, and
/// the rest of the core.
fn count(core: &str) -> Option<(usize, &str)> {
    [("two ", 2), ("three ", 3), ("four ", 4)]
        .iter()
        .find_map(|&(word, n)| core.strip_prefix(word).map(|rest| (n, rest)))
}

/// Encoded ranges of the words in `start..end`: whitespace-separated, with
/// every `,` a word of its own.
pub(crate) fn words(text: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut push = |word_start: usize, word_end: usize| {
        let mut s = word_start;
        let mut e = word_end;
        let mut trailing = 0;
        while s < e && text.as_bytes()[s] == b',' {
            out.push((s, s + 1));
            s += 1;
        }
        while e > s && text.as_bytes()[e - 1] == b',' {
            e -= 1;
            trailing += 1;
        }
        if s < e {
            out.push((s, e));
        }
        for offset in 0..trailing {
            out.push((e + offset, e + offset + 1));
        }
    };
    let mut word_start = None;
    for (offset, ch) in text[start..end].char_indices() {
        let at = start + offset;
        match (ch.is_whitespace(), word_start) {
            (true, Some(s)) => {
                push(s, at);
                word_start = None;
            }
            (false, None) => word_start = Some(at),
            _ => {}
        }
    }
    if let Some(s) = word_start {
        push(s, end);
    }
    out
}

/// The literal before a parameter: lowercased, without leading `,`/`and`,
/// trailing article words or any `optional`/`optionally`.
fn normalize_literal(words: &[String]) -> String {
    let mut words: Vec<String> = words
        .iter()
        .flat_map(|w| {
            w.split_whitespace()
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        })
        .filter(|w| w != "optional" && w != "optionally")
        .collect();
    let leading = words
        .iter()
        .take_while(|w| *w == "," || *w == "and")
        .count();
    words.drain(..leading);
    while words.last().is_some_and(|w| ARTICLES.contains(&w.as_str())) {
        words.pop();
    }
    let mut out = String::new();
    for word in words {
        if !out.is_empty() && word != "," {
            out.push(' ');
        }
        out.push_str(&word);
    }
    out
}

/// The `)` closing a `(` opened just before `start`, nested parentheses
/// balanced and code ignored.
fn closing_paren(enc: &Encoded, start: usize) -> Option<usize> {
    let mut depth = 1usize;
    for (offset, byte) in enc.text.as_bytes()[start..].iter().enumerate() {
        let at = start + offset;
        if enc.is_protected(at) {
            continue;
        }
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

/// The intro's terminator after its last parameter, with the return type it
/// states; `Err` when the rest is not a terminator.
fn terminator(
    rest: &str,
    links: &[(String, TypeRef)],
    names: &NameResolver,
) -> Result<Option<TypeExpr>, ()> {
    let rest = rest.trim_end();
    if TERMINATORS.contains(&rest) {
        return Ok(None);
    }
    // A terminating sentence followed by notes on the parameters.
    if TERMINATORS.iter().any(|t| {
        t.ends_with('.')
            && rest
                .strip_prefix(t)
                .is_some_and(|notes| notes.starts_with(' '))
    }) {
        return Ok(None);
    }
    let stated = rest
        .strip_prefix(". They return ")
        .and_then(|r| r.strip_suffix('.'))
        .or_else(|| {
            let r = rest.strip_prefix(", which returns ")?;
            r.strip_suffix(':').or_else(|| r.strip_suffix('.'))
        })
        .ok_or(())?;
    let mut core = stated.trim();
    let mut stripped = false;
    while let Some((word, after)) = core.split_once(' ') {
        if !ARTICLES.contains(&word) {
            break;
        }
        core = after.trim_start();
        stripped = true;
    }
    parse_intro_type(core, links, names, stripped)
        .map(|(ty, _)| Some(ty))
        .ok_or(())
}

/// Types the sole untyped positional parameter by the dfn's `data-dfn-for`.
fn apply_dfn_for(
    params: &mut [Param],
    issues: &mut Vec<SignatureIssue>,
    owner: &str,
    names: &NameResolver,
) {
    let mut untyped = params.iter_mut().filter(|p| {
        p.type_basis == TypeBasis::Unknown && matches!(p.passing, Passing::Positional { .. })
    });
    let (Some(param), None) = (untyped.next(), untyped.next()) else {
        return;
    };
    let owner = normalize_owner(owner);
    let Some(key) = names.resolve(&owner) else {
        return;
    };
    param.ty = TypeExpr::Nominal {
        ty: TypeRef::Known(key),
        text: owner,
    };
    param.type_basis = TypeBasis::DfnFor;
    let untyped_issue = SignatureIssue::UntypedParam(param.name.clone());
    if let Some(at) = issues.iter().position(|issue| *issue == untyped_issue) {
        issues.remove(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::steps::extract_step_structure_from_document;
    use crate::state::intro::{algorithm_intros, IdIndex};
    use crate::state::ir::Expr;
    use crate::state::model::{
        Literal, Passing, Primitive, ReturnBasis, SignatureForm, SignatureIssue,
        TemplatePiece as P, TypeBasis,
    };
    use crate::state::testing::*;

    fn sigs(html: &str, spec: &str) -> Vec<Signature> {
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
            .filter_map(|i| to_signature(i, &names))
            .collect()
    }
    fn find<'a>(all: &'a [Signature], anchor: &str) -> &'a Signature {
        all.iter().find(|s| s.algorithm.anchor == anchor).unwrap()
    }
    fn lit(s: &str) -> P {
        P::Literal(s.into())
    }

    #[test]
    fn prose_type_descriptions_are_types_not_literals() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="q">queue a global task</dfn> on a <a href="#task-source">task source</a> <var>source</var>, with a <a href="#global-object">global object</a> <var>global</var> and a series of steps <var>steps</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="dims">determine dimensions</dfn> of <var>image</var> given a struct consisting of a width and a height, or null, <var>natural</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="fire">fire</dfn> named <var>e</var> at <var>target</var>, optionally using an <var>eventConstructor</var>, and a <var>legacy flag</var>:</p><ol><li><p>Return.</p></li></ol></div>"##;
        let all = sigs(html, "HTML");
        let q = find(&all, "q");
        assert_eq!(
            q.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                lit("on"),
                P::Slot(0),
                lit("with"),
                P::Slot(1),
                P::ListSep,
                P::Slot(2)
            ]
        );
        assert_eq!(q.params[2].type_text, "series of steps");
        assert_eq!(
            q.params[2].ty,
            TypeExpr::Opaque {
                text: "series of steps".into()
            }
        );
        let dims = find(&all, "dims");
        assert_eq!(
            dims.template.as_ref().unwrap().pieces,
            vec![P::Callee, lit("of"), P::Slot(0), lit("given"), P::Slot(1)]
        );
        assert_eq!(
            dims.params[1].type_text,
            "struct consisting of a width and a height, or null"
        );
        let fire = find(&all, "fire");
        assert_eq!(
            fire.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                lit("named"),
                P::Slot(0),
                lit("at"),
                P::Slot(1),
                lit("using"),
                P::Slot(2),
                P::ListSep,
                P::Slot(3)
            ]
        );
        assert_eq!(fire.params[2].ty, TypeExpr::Unknown);
        assert!(fire.params[2].optional && fire.params[3].optional);
    }

    #[test]
    fn variables_after_the_intro_sentence_are_not_parameters() {
        let html = r##"<p>To <dfn id="fetch">fetch a classic script</dfn> given a <a href="https://url.spec.whatwg.org/#concept-url">URL</a> <var>url</var>, and an algorithm <var>onComplete</var>, run these steps. <var>onComplete</var> must be an algorithm accepting null.</p><ol><li><p>Return.</p></li></ol>
<p>To <dfn id="step">apply the step</dfn> given a non-negative integer <var>step</var>:</p><ol><li><p>Return.</p></li></ol>"##;
        let all = sigs(html, "HTML");
        let s = find(&all, "fetch");
        let names: Vec<&str> = s.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["url", "onComplete"]);
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![P::Callee, lit("given"), P::Slot(0), P::ListSep, P::Slot(1)]
        );
        assert!(!s
            .issues
            .iter()
            .any(|i| matches!(i, SignatureIssue::DuplicateParam(_))));
        assert!(!s
            .issues
            .iter()
            .any(|i| matches!(i, SignatureIssue::UnparsedIntroTail(_))));
        let step = find(&all, "step");
        assert_eq!(step.params[0].ty, TypeExpr::Primitive(Primitive::Integer));
        assert_eq!(step.params[0].type_basis, TypeBasis::Explicit);
    }

    #[test]
    fn which_clause_after_a_parameter_is_not_the_next_literal() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="report">report an exception</dfn> <var>exception</var> which is a JavaScript value, for a particular <a href="#global-object">global object</a> <var>global</var>:</p><ol><li><p>Return.</p></li></ol></div>"##;
        let all = sigs(html, "HTML");
        let s = find(&all, "report");
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![P::Callee, P::Slot(0), lit("for"), P::Slot(1)]
        );
        assert_eq!(s.params[1].type_text, "global object");
    }

    #[test]
    fn g1_navigate_template_params_and_defaults() {
        let all = sigs(NAV_HTML, "HTML");
        let s = find(&all, "navigate");
        assert_eq!(s.form, SignatureForm::To);
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                P::Slot(0),
                lit("to"),
                P::Slot(1),
                lit("using"),
                P::Slot(2),
                P::NamedGroup("with".into())
            ]
        );
        let rows: Vec<_> = s
            .params
            .iter()
            .map(|p| (p.name.as_str(), ty(&p.ty), p.passing.clone(), p.optional))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "navigable",
                    "HTML#navigable".to_string(),
                    Passing::Positional { index: 0 },
                    false
                ),
                (
                    "url",
                    "URL#concept-url".to_string(),
                    Passing::Positional { index: 1 },
                    false
                ),
                (
                    "sourceDocument",
                    "idl:Document | null".to_string(),
                    Passing::Positional { index: 2 },
                    true
                ),
                (
                    "exceptionsEnabled",
                    "boolean".to_string(),
                    Passing::Named,
                    true
                ),
                (
                    "historyHandling",
                    "idl:NavigationHistoryBehavior".to_string(),
                    Passing::Named,
                    true
                ),
                (
                    "referrerPolicy",
                    "HTML#referrer-policy".to_string(),
                    Passing::Named,
                    true
                ),
            ]
        );
        assert_eq!(s.params[2].default, Some(Expr::Literal(Literal::Null)));
        assert_eq!(
            s.params[2].anchor.as_ref().unwrap().anchor,
            "source-browsing-context"
        );
        assert_eq!(
            s.params[3].default,
            Some(Expr::Literal(Literal::Bool(false)))
        );
        assert_eq!(
            s.params[3].anchor.as_ref().unwrap().anchor,
            "exceptions-enabled"
        );
        assert!(
            matches!(&s.params[4].default, Some(Expr::EnumValue { text, target: Some(t) }) if text == "auto" && t.anchor == "navigationhistorybehavior-auto")
        );
        assert_eq!(
            s.params[5].default,
            Some(Expr::Literal(Literal::String(String::new())))
        );
        assert_eq!(s.params[1].type_text, "URL");
        assert_eq!(s.returns, None);
        assert!(s.issues.is_empty(), "{:?}", s.issues);
    }

    #[test]
    fn g3_bikeshed_insert_and_union_with_leading_null() {
        let all = sigs(INSERT_DOM, "DOM");
        let s = find(&all, "concept-node-insert");
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                P::Slot(0),
                lit("into"),
                P::Slot(1),
                lit("before"),
                P::Slot(2),
                P::NamedGroup("with".into())
            ]
        );
        assert_eq!(ty(&s.params[0].ty), "idl:Node");
        assert_eq!(ty(&s.params[2].ty), "null | idl:Node");
        assert_eq!(
            (s.params[3].name.as_str(), s.params[3].passing.clone()),
            ("suppressObservers", Passing::Named)
        );
        assert_eq!(
            s.params[3].anchor.as_ref().unwrap().anchor,
            "insert-suppressobservers"
        );
    }

    #[test]
    fn given_lists_and_sticky_optionally() {
        let all = sigs(CREATE_ELEMENT_DOM, "DOM");
        let s = find(&all, "concept-create-element");
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                lit("given"),
                P::Slot(0),
                P::ListSep,
                P::Slot(1),
                P::ListSep,
                P::Slot(2),
                P::ListSep,
                P::Slot(3),
                P::ListSep,
                P::Slot(4),
                P::ListSep,
                P::Slot(5)
            ]
        );
        let rows: Vec<_> = s.params.iter().map(|p| (ty(&p.ty), p.optional)).collect();
        assert_eq!(
            rows,
            vec![
                ("DOM#concept-document".to_string(), false),
                ("string".to_string(), false),
                ("string | null".to_string(), false),
                ("string | null".to_string(), true),
                ("string | null".to_string(), true),
                ("boolean".to_string(), true)
            ]
        );
        assert_eq!(
            s.params[5].default,
            Some(Expr::Literal(Literal::Bool(false)))
        );
    }

    #[test]
    fn head_words_name_resolution_and_opaque_types() {
        let all = sigs(AUTODIR_HTML, "HTML");
        let s = find(&all, "auto-directionality");
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![
                P::Head("compute the".into()),
                P::Callee,
                lit("given"),
                P::Slot(0)
            ]
        );
        assert_eq!(
            (ty(&s.params[0].ty), s.params[0].type_basis),
            ("idl:Element".to_string(), TypeBasis::NameResolved)
        );
        let ui = find(&all, "use-objects");
        assert_eq!(
            ty(&ui.params[0].ty),
            "opaque(object implementing CanvasUserInterface)"
        );
        assert_eq!(ui.issues, vec![SignatureIssue::OpaqueType("ui".into())]);
        assert!(all
            .iter()
            .all(|s| s.algorithm.anchor != "rules-to-parse-a-date-string"));
    }

    #[test]
    fn counted_type_applies_to_that_many_listed_parameters() {
        let html = r##"<div data-algorithm=""><p>To <dfn id="check">check</dfn>, given two <a href="#concept-origin">origins</a> <var>a</var> and <var>b</var>, and <var>c</var>:</p><ol><li><p>Return.</p></li></ol></div>
<div data-algorithm=""><p>To <dfn id="report">report</dfn> given three <a href="#concept-url">URLs</a> <var>x</var>, <var>y</var> and <var>z</var>:</p><ol><li><p>Return.</p></li></ol></div>"##;
        let all = sigs(html, "HTML");
        let s = find(&all, "check");
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                lit("given"),
                P::Slot(0),
                P::ListSep,
                P::Slot(1),
                P::ListSep,
                P::Slot(2)
            ]
        );
        assert_eq!(ty(&s.params[0].ty), "HTML#concept-origin");
        assert_eq!(s.params[0].type_text, "two origins");
        assert_eq!(
            (&s.params[1].ty, s.params[1].type_basis),
            (&s.params[0].ty, TypeBasis::Explicit)
        );
        assert_eq!(s.params[2].ty, TypeExpr::Unknown);
        assert_eq!(s.issues, vec![SignatureIssue::UntypedParam("c".into())]);
        let r = find(&all, "report");
        assert!(r.params.iter().all(|p| ty(&p.ty) == "HTML#concept-url"));
        assert!(r.issues.is_empty());
    }

    #[test]
    fn when_steps_say_and_given_list_forms() {
        let all = sigs(PREPARE_EVENT_HTML, "HTML");
        let s = find(&all, "prepare-an-event");
        assert_eq!(s.form, SignatureForm::WhenStepsSay);
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                lit("named"),
                P::Slot(0),
                lit("for"),
                P::Slot(1),
                lit("with"),
                P::Slot(2)
            ]
        );
        assert_eq!(ty(&s.params[1].ty), "HTML#text-track-cue");
        assert_eq!(ty(&s.params[2].ty), "opaque(time)");
        assert!(s
            .issues
            .contains(&SignatureIssue::UntypedParam("event".into())));
        let all = sigs(GIVENLIST_HTML, "HTML");
        let s = find(&all, "inner-navigate-event-firing-algorithm");
        assert_eq!(s.form, SignatureForm::GivenList);
        assert_eq!(
            s.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                lit("given"),
                P::Slot(0),
                P::ListSep,
                P::Slot(1),
                P::ListSep,
                P::Slot(2)
            ]
        );
        assert_eq!(
            s.params.iter().map(|p| ty(&p.ty)).collect::<Vec<_>>(),
            [
                "idl:NavigationType",
                "idl:NavigationDestination",
                "HTML#user-navigation-involvement"
            ]
        );
    }

    #[test]
    fn dfn_for_types_the_sole_untyped_positional_parameter_only() {
        let html = format!("{EVENT_DOM}<div class=\"algorithm\"><p>To <dfn data-dfn-for=\"Event\" id=\"reset-event\">reset</dfn> an <var>event</var> given a boolean <var>b</var>:</p><ol><li><p>Return.</p></li></ol></div>");
        let all = sigs(&html, "DOM");
        let reset = find(&all, "reset-event");
        assert_eq!(
            (ty(&reset.params[0].ty), reset.params[0].type_basis),
            ("idl:Event".to_string(), TypeBasis::DfnFor)
        );
        assert!(reset.issues.is_empty());
        let init = find(&all, "concept-event-initialize");
        assert!(
            init.params
                .iter()
                .all(|p| p.type_basis == TypeBasis::Unknown),
            "four untyped positionals: hint ignored"
        );
        assert_eq!(
            init.template.as_ref().unwrap().pieces,
            vec![
                P::Callee,
                P::Slot(0),
                lit("with"),
                P::Slot(1),
                P::ListSep,
                P::Slot(2),
                P::ListSep,
                P::Slot(3)
            ]
        );
    }

    #[test]
    fn stated_return_types_and_step_terminators() {
        let html = r##"<p>To <dfn id="f">f</dfn> given a string <var>s</var>, which returns a boolean:</p><ol><li><p>Return true.</p></li></ol>
<p>To <dfn id="g">g</dfn> a string <var>s</var>. They return a <a href="https://infra.spec.whatwg.org/#list">list</a>.</p><ol><li><p>Return « ».</p></li></ol>
<p>To <dfn id="h">h</dfn> given a string <var>s</var>, perform the following steps.</p><ol><li><p>Return.</p></li></ol>
<p>To <dfn id="k">k</dfn> given a string <var>s</var>, which returns a thing that is odd:</p><ol><li><p>Return.</p></li></ol>"##;
        let all = sigs(html, "HTML");
        let f = find(&all, "f");
        assert_eq!(
            f.returns.as_ref().map(|r| (ty(&r.ty), r.basis)),
            Some(("boolean".to_string(), ReturnBasis::Intro))
        );
        assert!(f.issues.is_empty(), "{:?}", f.issues);
        let g = find(&all, "g");
        assert_eq!(g.returns.as_ref().map(|r| ty(&r.ty)), Some("list".into()));
        assert!(g.issues.is_empty(), "{:?}", g.issues);
        let h = find(&all, "h");
        assert_eq!((h.returns.as_ref(), h.issues.len()), (None, 0));
        let k = find(&all, "k");
        assert_eq!(k.returns, None);
        assert_eq!(
            k.issues,
            vec![SignatureIssue::UnparsedIntroTail(
                ", which returns a thing that is odd:".into()
            )]
        );
    }

    #[test]
    fn duplicate_and_unterminated_intros_report_issues() {
        let html = r##"<p>To <dfn id="dup">dup</dfn> given a string <var>x</var> and a string <var>x</var> (default "a"</p><ol><li><p>Return.</p></li></ol>
<p>To <dfn id="mid">mid</dfn> given a string <var>y</var> and then</p><ol><li><p>Return.</p></li></ol>"##;
        let all = sigs(html, "HTML");
        let dup = find(&all, "dup");
        assert_eq!(dup.params.len(), 2);
        assert!(dup
            .issues
            .contains(&SignatureIssue::DuplicateParam("x".into())));
        assert!(dup
            .issues
            .iter()
            .any(|i| matches!(i, SignatureIssue::UnparsedIntroTail(_))));
        let mid = find(&all, "mid");
        assert_eq!(
            mid.issues,
            vec![SignatureIssue::UnparsedIntroTail("and then".into())]
        );
    }
}
