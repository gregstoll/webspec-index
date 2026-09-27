//! IDL member declarations and the signatures of IDL steps (§8.1): method,
//! getter, setter and constructor steps typed from the IDL block declaring them.
use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

use regex::Regex;

use crate::model::ParsedIdlDefinition;
use crate::parse::idl_defs::normalize_owner;
use crate::parse::steps::TextSpan;
use crate::state::intro::Intro;
use crate::state::ir::Expr;
use crate::state::model::{
    Literal, ObjectModel, Param, Passing, ReturnBasis, ReturnType, Signature, SignatureForm,
    SignatureIssue, SuperBasis, TypeBasis, TypeExpr, TypeKey, TypeRef,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdlArg {
    name: String,
    ty: String,
    optional: bool,
    default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdlMember {
    Constructor {
        args: Vec<IdlArg>,
    },
    Attribute {
        name: String,
        ty: String,
    },
    Operation {
        name: String,
        ret: String,
        args: Vec<IdlArg>,
    },
}

/// The members every interface and mixin declares across all IDL blocks of a
/// spec, the mixins each interface includes, and the declared IDL type names.
pub(crate) struct IdlMembers {
    members: HashMap<String, Vec<IdlMember>>,
    includes: HashMap<String, Vec<String>>,
    known: BTreeSet<String>,
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

impl IdlMembers {
    #[allow(dead_code)]
    pub(crate) fn new(defs: &[ParsedIdlDefinition], model: &ObjectModel) -> Self {
        static BLOCK: OnceLock<Regex> = OnceLock::new();
        static INCLUDES: OnceLock<Regex> = OnceLock::new();
        static DECLARED: OnceLock<Regex> = OnceLock::new();
        static TYPEDEF: OnceLock<Regex> = OnceLock::new();
        let mut members: HashMap<String, Vec<IdlMember>> = HashMap::new();
        let mut includes: HashMap<String, Vec<String>> = HashMap::new();
        let mut known = BTreeSet::new();
        let texts: BTreeSet<&str> = defs.iter().filter_map(|d| d.idl_text.as_deref()).collect();
        for raw in texts {
            let text = strip_comments(raw);
            for caps in regex(
                &DECLARED,
                r"\b(?:interface|dictionary|enum|callback)(?:\s+(?:mixin|interface))?\s+([A-Za-z_]\w*)",
            )
            .captures_iter(&text)
            {
                known.insert(identifier(&caps[1]));
            }
            for caps in regex(&TYPEDEF, r"\btypedef\s[^;]*?([A-Za-z_]\w*)\s*;").captures_iter(&text)
            {
                known.insert(identifier(&caps[1]));
            }
            for caps in regex(
                &INCLUDES,
                r"\b([A-Za-z_]\w*)\s+includes\s+([A-Za-z_]\w*)\s*;",
            )
            .captures_iter(&text)
            {
                push_unique(
                    includes.entry(identifier(&caps[1])).or_default(),
                    identifier(&caps[2]),
                );
            }
            for caps in regex(
                &BLOCK,
                r"\binterface(?:\s+mixin)?\s+([A-Za-z_]\w*)\s*(?::\s*[A-Za-z_]\w*\s*)?\{",
            )
            .captures_iter(&text)
            {
                let open = caps.get(0).unwrap().end();
                let Some(body) = block_body(&text, open) else {
                    continue;
                };
                let list = members.entry(identifier(&caps[1])).or_default();
                list.extend(
                    split_top(body, ';')
                        .iter()
                        .filter_map(|member| parse_member(member)),
                );
            }
        }
        for def in &model.types {
            let TypeKey::Idl(name) = &def.key else {
                continue;
            };
            known.insert(name.clone());
            for edge in &def.supertypes {
                if let (SuperBasis::IdlIncludes, TypeKey::Idl(mixin)) = (&edge.basis, &edge.target)
                {
                    push_unique(includes.entry(name.clone()).or_default(), mixin.clone());
                }
            }
        }
        Self {
            members,
            includes,
            known,
        }
    }

    /// The interface's own members (all partials), then those of every mixin it includes.
    fn lookup<'a>(&'a self, interface: &'a str) -> impl Iterator<Item = &'a IdlMember> + 'a {
        let mixins = self
            .includes
            .get(interface)
            .into_iter()
            .flatten()
            .map(String::as_str);
        std::iter::once(interface)
            .chain(mixins)
            .filter_map(|name| self.members.get(name))
            .flatten()
    }
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

/// An IDL identifier without its escaping underscore.
fn identifier(name: &str) -> String {
    name.strip_prefix('_').unwrap_or(name).to_string()
}

fn strip_comments(text: &str) -> String {
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    regex(&COMMENT, r"(?s)//[^\n]*|/\*.*?\*/")
        .replace_all(text, "")
        .into_owned()
}

/// The text between the `{` ending at `open` and its matching `}`.
fn block_body(text: &str, open: usize) -> Option<&str> {
    let mut depth = 1usize;
    for (at, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open..open + at]);
                }
            }
            _ => {}
        }
    }
    None
}

/// `text` split at `sep` outside brackets and string literals, pieces trimmed,
/// empty pieces dropped.
fn split_top(text: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quoted = false;
    let mut start = 0;
    for (at, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            _ if quoted => {}
            '<' | '(' | '[' | '{' => depth += 1,
            '>' | ')' | ']' | '}' => depth -= 1,
            _ if c == sep && depth == 0 => {
                out.push(text[start..at].trim());
                start = at + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(text[start..].trim());
    out.retain(|piece| !piece.is_empty());
    out
}

/// `text` without leading extended attribute lists `[...]`.
fn strip_ext_attrs(mut text: &str) -> &str {
    loop {
        text = text.trim_start();
        if !text.starts_with('[') {
            return text;
        }
        let mut depth = 0i32;
        let mut end = None;
        for (at, c) in text.char_indices() {
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(at + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(end) => text = &text[end..],
            None => return text,
        }
    }
}

fn strip_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(word)?;
    rest.starts_with(char::is_whitespace)
        .then(|| rest.trim_start())
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `TYPE NAME` split at the last whitespace; `None` unless NAME is an identifier
/// and TYPE is non-empty.
fn type_and_name(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let at = text.rfind(char::is_whitespace)?;
    let (ty, name) = (text[..at].trim(), text[at..].trim());
    (!ty.is_empty() && is_identifier(name)).then(|| (collapse(ty), identifier(name)))
}

fn parse_member(text: &str) -> Option<IdlMember> {
    let mut text = strip_ext_attrs(text);
    if let Some(rest) = text.strip_prefix("constructor") {
        let args = rest.trim().strip_prefix('(')?.strip_suffix(')')?;
        return Some(IdlMember::Constructor {
            args: parse_args(args),
        });
    }
    let mut attribute = false;
    loop {
        let before = text;
        for word in [
            "static",
            "readonly",
            "inherit",
            "getter",
            "setter",
            "deleter",
            "stringifier",
        ] {
            if let Some(rest) = strip_word(text, word) {
                text = rest;
            }
        }
        if let Some(rest) = strip_word(text, "attribute") {
            text = rest;
            attribute = true;
        }
        if text == before {
            break;
        }
    }
    if attribute {
        let (ty, name) = type_and_name(text)?;
        return Some(IdlMember::Attribute { name, ty });
    }
    let args_end = text.strip_suffix(')')?.len();
    let mut depth = 0i32;
    let mut args_start = None;
    for (at, c) in text[..args_end].char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' if depth == 0 => {
                args_start = Some(at);
                break;
            }
            '(' => depth -= 1,
            _ => {}
        }
    }
    let args_start = args_start?;
    let (ret, name) = type_and_name(&text[..args_start])?;
    Some(IdlMember::Operation {
        name,
        ret,
        args: parse_args(&text[args_start + 1..args_end]),
    })
}

fn parse_args(text: &str) -> Vec<IdlArg> {
    split_top(text, ',')
        .into_iter()
        .filter_map(|arg| {
            let mut arg = strip_ext_attrs(arg);
            let optional = match strip_word(arg, "optional") {
                Some(rest) => {
                    arg = strip_ext_attrs(rest);
                    true
                }
                None => false,
            };
            let (decl, default) = match arg.split_once('=') {
                Some((decl, default)) => (decl, Some(default.trim().to_string())),
                None => (arg, None),
            };
            let (ty, name) = type_and_name(decl)?;
            let ty = ty.strip_suffix("...").unwrap_or(&ty).trim_end().to_string();
            Some(IdlArg {
                name,
                ty,
                optional,
                default,
            })
        })
        .collect()
}

fn default_expr(text: &str) -> Expr {
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    let literal = match text {
        "true" => Some(Literal::Bool(true)),
        "false" => Some(Literal::Bool(false)),
        "null" => Some(Literal::Null),
        _ if regex(
            &NUMBER,
            r"^-?(?:0[xX][0-9A-Fa-f]+|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|\.\d+|Infinity|NaN)$",
        )
        .is_match(text) =>
        {
            Some(Literal::Number(text.to_string()))
        }
        _ => text
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .map(|s| Literal::String(s.to_string())),
    };
    match literal {
        Some(literal) => Expr::Literal(literal),
        None => Expr::Opaque {
            text: text.to_string(),
        },
    }
}

/// The type an IDL type string denotes: declared IDL names are nominal, a
/// trailing `?` adds `null`, everything else stays IDL text.
#[allow(dead_code)]
pub(crate) fn idl_type(text: &str, known: &BTreeSet<String>) -> TypeExpr {
    let text = collapse(text);
    if let Some(inner) = text.strip_suffix('?') {
        return TypeExpr::Union(vec![idl_type(inner, known), TypeExpr::Null]);
    }
    if known.contains(&text) {
        return TypeExpr::Nominal {
            ty: TypeRef::Known(TypeKey::Idl(text.clone())),
            text,
        };
    }
    TypeExpr::Idl { text }
}

/// The member a steps intro names, the IDL construct it is steps for, and its owner.
struct IntroMember {
    role: Role,
    interface: String,
    member: String,
    params: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Method,
    Getter,
    Setter,
    Constructor,
}

fn intro_member(intro: &Intro) -> Option<IntroMember> {
    static INTRO: OnceLock<Regex> = OnceLock::new();
    let caps = regex(
        &INTRO,
        r"^The (?:new )?(?P<member>.+?) (?P<role>method|getter|setter|constructor|attribute)(?: steps are|, when invoked, must| must return| steps must)",
    )
    .captures(&intro.source.text)?;
    let role = match &caps["role"] {
        "method" => Role::Method,
        "getter" | "attribute" => Role::Getter,
        "setter" => Role::Setter,
        _ => Role::Constructor,
    };
    let raw = if intro.dfn_text.is_empty() {
        caps["member"].to_string()
    } else {
        intro.dfn_text.clone()
    };
    let text = raw.replace('`', "");
    let text = text.trim();
    let text = strip_word(text, "new").unwrap_or(text);
    let (name, params) = match text.split_once('(') {
        Some((name, rest)) => {
            let inside = rest.rsplit_once(')').map_or(rest, |(inside, _)| inside);
            let params = split_top(inside, ',')
                .into_iter()
                .map(|p| p.trim_end_matches("...").trim().to_string())
                .collect();
            (name.trim().to_string(), params)
        }
        None => (text.to_string(), Vec::new()),
    };
    let interface = match (&intro.dfn_for, role) {
        (Some(owner), _) => normalize_owner(owner),
        (None, Role::Constructor) => name.clone(),
        (None, _) => String::new(),
    };
    if interface.is_empty() || name.is_empty() {
        return None;
    }
    Some(IntroMember {
        role,
        interface,
        member: name,
        params,
    })
}

/// The overload whose argument names equal `params`, else the first candidate.
fn pick_overload<'a, T>(
    candidates: &'a [(T, &'a [IdlArg])],
    params: &[String],
) -> Option<&'a (T, &'a [IdlArg])> {
    candidates
        .iter()
        .find(|(_, args)| args.iter().map(|a| &a.name).eq(params.iter()))
        .or(candidates.first())
}

/// The signature of an IDL method, getter, setter or constructor steps intro;
/// `None` when the intro is no IDL steps intro or names no interface.
#[allow(dead_code)]
pub(crate) fn idl_signature(intro: &Intro, members: &IdlMembers) -> Option<Signature> {
    if intro.ecmarkup {
        return None;
    }
    let found = intro_member(intro)?;
    let interface = found.interface.clone();
    let member = found.member.clone();
    let known = &members.known;

    let (form, params, args, returns): (_, Vec<String>, Option<Vec<IdlArg>>, _) = match found.role {
        Role::Method => {
            let candidates: Vec<(&str, &[IdlArg])> = members
                .lookup(&interface)
                .filter_map(|m| match m {
                    IdlMember::Operation { name, ret, args } if *name == member => {
                        Some((ret.as_str(), args.as_slice()))
                    }
                    _ => None,
                })
                .collect();
            let chosen = pick_overload(&candidates, &found.params);
            (
                SignatureForm::IdlMethod {
                    interface: interface.clone(),
                    member: member.clone(),
                },
                found.params.clone(),
                chosen.map(|(_, args)| args.to_vec()),
                chosen.map(|(ret, _)| ReturnType {
                    ty: idl_type(ret, known),
                    basis: ReturnBasis::Idl,
                }),
            )
        }
        Role::Constructor => {
            let candidates: Vec<((), &[IdlArg])> = members
                .lookup(&interface)
                .filter_map(|m| match m {
                    IdlMember::Constructor { args } => Some(((), args.as_slice())),
                    _ => None,
                })
                .collect();
            (
                SignatureForm::IdlConstructor {
                    interface: interface.clone(),
                },
                found.params.clone(),
                pick_overload(&candidates, &found.params).map(|(_, args)| args.to_vec()),
                None,
            )
        }
        Role::Getter | Role::Setter => {
            let attribute = members.lookup(&interface).find_map(|m| match m {
                IdlMember::Attribute { name, ty } if *name == member => Some(ty.clone()),
                _ => None,
            });
            if found.role == Role::Getter {
                (
                    SignatureForm::IdlGetter {
                        interface: interface.clone(),
                        member: member.clone(),
                    },
                    Vec::new(),
                    attribute.as_ref().map(|_| Vec::new()),
                    attribute.map(|ty| ReturnType {
                        ty: idl_type(&ty, known),
                        basis: ReturnBasis::Idl,
                    }),
                )
            } else {
                let name = intro
                    .vars
                    .first()
                    .map_or_else(|| "value".to_string(), |v| v.name.clone());
                (
                    SignatureForm::IdlSetter {
                        interface: interface.clone(),
                        member: member.clone(),
                    },
                    vec![name.clone()],
                    attribute.map(|ty| {
                        vec![IdlArg {
                            name,
                            ty,
                            optional: false,
                            default: None,
                        }]
                    }),
                    None,
                )
            }
        }
    };

    let params = params
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let var = intro.vars.iter().find(|v| v.name == *name);
            let arg = args.as_ref().and_then(|args| {
                args.iter()
                    .find(|a| a.name == *name)
                    .or_else(|| args.get(index))
            });
            let (ty, type_text, type_basis, optional, default) = match arg {
                Some(arg) => (
                    idl_type(&arg.ty, known),
                    arg.ty.clone(),
                    TypeBasis::Explicit,
                    arg.optional,
                    arg.default.as_deref().map(default_expr),
                ),
                None => (
                    TypeExpr::Unknown,
                    String::new(),
                    TypeBasis::Unknown,
                    false,
                    None,
                ),
            };
            Param {
                name: name.clone(),
                anchor: var.and_then(|v| v.anchor.clone()),
                ty,
                type_text,
                type_basis,
                optional,
                default,
                passing: Passing::Positional {
                    index: index as u32,
                },
                span: var.map_or(TextSpan { start: 0, end: 0 }, |v| v.span),
            }
        })
        .collect();

    Some(Signature {
        algorithm: intro.source.subject.clone(),
        source_id: intro.source.id.clone(),
        form,
        this: Some(TypeExpr::Nominal {
            ty: TypeRef::Known(TypeKey::Idl(interface.clone())),
            text: interface,
        }),
        params,
        returns,
        template: None,
        issues: if args.is_none() {
            vec![SignatureIssue::IdlMemberNotFound]
        } else {
            Vec::new()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::steps::extract_step_structure_from_document;
    use crate::state::intro::{algorithm_intros, IdIndex};
    use crate::state::ir::Expr;
    use crate::state::model::{
        Literal, Passing, ReturnBasis, SignatureForm, SignatureIssue, TypeBasis,
    };
    use crate::state::testing::*;

    const MORE_IDL: &str = r##"<pre class="idl">interface <dfn data-dfn-type="interface" id="interface-abortsignal">AbortSignal</dfn> {
  constructor(optional DOMString label = "");
  readonly attribute boolean aborted;
  attribute any reason;
  undefined add(DOMString a);
  undefined add(DOMString a, long b);
};
interface mixin <dfn data-dfn-type="interface" id="parentnode">ParentNode</dfn> {
  undefined append((Node or DOMString)... nodes);
};
AbortSignal includes ParentNode;</pre>
<div class="algorithm"><p>The <dfn data-dfn-for="AbortSignal" data-dfn-type="attribute" id="dom-abortsignal-aborted"><code>aborted</code></dfn> getter steps are:</p><ol><li><p>Return true.</p></li></ol></div>
<div class="algorithm"><p>The <dfn data-dfn-for="AbortSignal" data-dfn-type="attribute" id="dom-abortsignal-reason"><code>reason</code></dfn> setter steps are:</p><ol><li><p>Return.</p></li></ol></div>
<div class="algorithm"><p>The <dfn data-dfn-type="constructor" id="dom-abortsignal-abortsignal"><code>new AbortSignal(<var>label</var>)</code></dfn> constructor steps are:</p><ol><li><p>Return.</p></li></ol></div>
<div class="algorithm"><p>The <dfn data-dfn-for="AbortSignal" data-dfn-type="method" id="dom-abortsignal-add"><code>add(<var>a</var>, <var>b</var>)</code></dfn> method steps are:</p><ol><li><p>Return.</p></li></ol></div>
<div class="algorithm"><p>The <dfn data-dfn-for="AbortSignal" data-dfn-type="method" id="dom-parentnode-append"><code>append(<var>nodes</var>)</code></dfn> method steps are:</p><ol><li><p>Return.</p></li></ol></div>
<div class="algorithm"><p>The <dfn data-dfn-for="AbortSignal" data-dfn-type="method" id="dom-abortsignal-missing"><code>missing(<var>q</var>)</code></dfn> method steps are:</p><ol><li><p>Return.</p></li></ol></div>"##;

    fn idl_sigs(html: &str) -> Vec<Signature> {
        let document = scraper::Html::parse_document(html);
        let structure =
            extract_step_structure_from_document(&document, "DOM", base_url("DOM"), "hash:t");
        let state = extract_html(html, "DOM");
        let defs = crate::parse::idl_defs::extract_idl_definitions(&document);
        let members = IdlMembers::new(&defs, &state.model);
        let index = IdIndex::new(&document);
        algorithm_intros(
            &document,
            &index,
            "DOM",
            base_url("DOM"),
            "hash:t",
            &structure,
        )
        .iter()
        .filter_map(|i| idl_signature(i, &members))
        .collect()
    }
    fn find<'a>(all: &'a [Signature], anchor: &str) -> &'a Signature {
        all.iter().find(|s| s.algorithm.anchor == anchor).unwrap()
    }

    #[test]
    fn g4_init_event_from_the_idl_declaration() {
        let all = idl_sigs(EVENT_DOM);
        let s = find(&all, "dom-event-initevent");
        assert_eq!(
            s.form,
            SignatureForm::IdlMethod {
                interface: "Event".into(),
                member: "initEvent".into()
            }
        );
        assert_eq!(ty(s.this.as_ref().unwrap()), "idl:Event");
        let rows: Vec<_> = s
            .params
            .iter()
            .map(|p| (p.name.as_str(), ty(&p.ty), p.optional, p.default.clone()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("type", "idl-type:DOMString".to_string(), false, None),
                (
                    "bubbles",
                    "idl-type:boolean".to_string(),
                    true,
                    Some(Expr::Literal(Literal::Bool(false)))
                ),
                (
                    "cancelable",
                    "idl-type:boolean".to_string(),
                    true,
                    Some(Expr::Literal(Literal::Bool(false)))
                ),
            ]
        );
        for (index, p) in s.params.iter().enumerate() {
            assert_eq!(p.type_basis, TypeBasis::Explicit);
            assert_eq!(
                p.passing,
                Passing::Positional {
                    index: index as u32
                }
            );
            assert_eq!(p.span, TextSpan { start: 0, end: 0 });
        }
        assert_eq!(s.params[1].type_text, "boolean");
        let r = s.returns.as_ref().unwrap();
        assert_eq!(
            (ty(&r.ty), r.basis),
            ("idl-type:undefined".to_string(), ReturnBasis::Idl)
        );
        assert!(s.template.is_none() && s.issues.is_empty());
    }

    #[test]
    fn getters_setters_constructors_overloads_and_mixins() {
        let html = format!("{INSERT_DOM}{MORE_IDL}");
        let all = idl_sigs(&html);
        let getter = find(&all, "dom-abortsignal-aborted");
        assert_eq!(
            getter.form,
            SignatureForm::IdlGetter {
                interface: "AbortSignal".into(),
                member: "aborted".into()
            }
        );
        assert_eq!(ty(&getter.returns.as_ref().unwrap().ty), "idl-type:boolean");
        let setter = find(&all, "dom-abortsignal-reason");
        assert_eq!(
            (setter.params[0].name.as_str(), ty(&setter.params[0].ty)),
            ("value", "idl-type:any".to_string())
        );
        let ctor = find(&all, "dom-abortsignal-abortsignal");
        assert_eq!(
            ctor.form,
            SignatureForm::IdlConstructor {
                interface: "AbortSignal".into()
            }
        );
        assert_eq!(
            ctor.params[0].default,
            Some(Expr::Literal(Literal::String(String::new())))
        );
        let add = find(&all, "dom-abortsignal-add");
        assert_eq!(
            add.params.iter().map(|p| ty(&p.ty)).collect::<Vec<_>>(),
            ["idl-type:DOMString", "idl-type:long"]
        );
        let append = find(&all, "dom-parentnode-append");
        assert_eq!(ty(&append.params[0].ty), "idl-type:(Node or DOMString)");
        let missing = find(&all, "dom-abortsignal-missing");
        assert_eq!(missing.issues, vec![SignatureIssue::IdlMemberNotFound]);
        assert_eq!(missing.params[0].ty, TypeExpr::Unknown);

        assert_eq!(ty(getter.this.as_ref().unwrap()), "idl:AbortSignal");
        assert!(getter.params.is_empty() && getter.issues.is_empty());
        assert_eq!(
            setter.form,
            SignatureForm::IdlSetter {
                interface: "AbortSignal".into(),
                member: "reason".into()
            }
        );
        assert_eq!(
            setter.params[0].span,
            crate::parse::steps::TextSpan { start: 0, end: 0 }
        );
        assert!(setter.returns.is_none() && ctor.returns.is_none());
        assert_eq!(
            (ctor.params[0].name.as_str(), ctor.params[0].optional),
            ("label", true)
        );
        assert_eq!(
            add.params
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(append.issues.is_empty());
        assert_eq!(
            (
                missing.params[0].name.as_str(),
                missing.params[0].type_basis
            ),
            ("q", TypeBasis::Unknown)
        );
        assert!(missing.returns.is_none());
    }

    #[test]
    fn member_declarations_parse_qualifiers_attributes_and_defaults() {
        assert_eq!(
            parse_member("[NewObject] static Promise<undefined> go(optional [EnforceRange] unsigned long long n = 0x10, optional Opts o = {})"),
            Some(IdlMember::Operation {
                name: "go".into(),
                ret: "Promise<undefined>".into(),
                args: vec![
                    IdlArg { name: "n".into(), ty: "unsigned long long".into(), optional: true, default: Some("0x10".into()) },
                    IdlArg { name: "o".into(), ty: "Opts".into(), optional: true, default: Some("{}".into()) },
                ],
            })
        );
        assert_eq!(
            parse_member("[Reflect] inherit readonly attribute Node? _default"),
            Some(IdlMember::Attribute {
                name: "default".into(),
                ty: "Node?".into()
            })
        );
        assert_eq!(parse_member("getter DOMString (DOMString name)"), None);
        assert_eq!(
            default_expr("-1.5"),
            Expr::Literal(Literal::Number("-1.5".into()))
        );
        assert_eq!(default_expr("null"), Expr::Literal(Literal::Null));
        assert_eq!(default_expr("[]"), Expr::Opaque { text: "[]".into() });
    }

    #[test]
    fn idl_type_maps_known_names_and_nullables() {
        let known: BTreeSet<String> = ["Node".to_string()].into();
        assert_eq!(
            crate::state::testing::ty(&idl_type("Node?", &known)),
            "idl:Node | null"
        );
        assert_eq!(
            crate::state::testing::ty(&idl_type("unsigned  long", &known)),
            "idl-type:unsigned long"
        );
        assert_eq!(
            crate::state::testing::ty(&idl_type("sequence<Node>", &known)),
            "idl-type:sequence<Node>"
        );
    }
}
