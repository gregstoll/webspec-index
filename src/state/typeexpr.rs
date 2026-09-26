//! Type expression and initial value extraction (§6.4).
//!
//! `parse_type_phrase` is the core type parser used by `declared_type` and
//! stage C YAML processing.

use crate::parse::steps::AnchorTarget;
use crate::state::block::{sentences, BlockToken, Pattern};
use crate::state::model::{
    InfraKind, InitialValue, Literal, Primitive, TypeExpr, TypeKey, TypeRef,
};
use regex::Regex;
use std::ops::Range;
use std::sync::OnceLock;

fn regex(cell: &'static OnceLock<Regex>, source: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(source).unwrap())
}

// ── Primitive and Infra kind tables ──────────────────────────────────────────

fn primitive_phrase(s: &str) -> Option<Primitive> {
    match s {
        "boolean" => Some(Primitive::Boolean),
        "string" => Some(Primitive::String),
        "number" => Some(Primitive::Number),
        "integer" => Some(Primitive::Integer),
        "byte sequence" => Some(Primitive::ByteSequence),
        "scalar value string" => Some(Primitive::ScalarValueString),
        _ => None,
    }
}

fn infra_anchor_kind(anchor: &str) -> Option<InfraKind> {
    match anchor {
        "list" => Some(InfraKind::List),
        "ordered-set" => Some(InfraKind::OrderedSet),
        "ordered-map" => Some(InfraKind::OrderedMap),
        "map" => Some(InfraKind::Map),
        "tuple" => Some(InfraKind::Tuple),
        "struct" => Some(InfraKind::Struct),
        _ => None,
    }
}

/// True if the TypeRef points to an INFRA collection type; returns the kind.
fn infra_link_kind(ty: &TypeRef) -> Option<InfraKind> {
    let target = match ty {
        TypeRef::Unresolved(t) => t,
        TypeRef::Known(TypeKey::Anchor(t)) => t,
        _ => return None,
    };
    if target.spec != "INFRA" {
        return None;
    }
    infra_anchor_kind(&target.anchor)
}

fn infra_word_kind(s: &str) -> Option<InfraKind> {
    match s {
        "list" => Some(InfraKind::List),
        "ordered set" => Some(InfraKind::OrderedSet),
        "ordered map" => Some(InfraKind::OrderedMap),
        "map" => Some(InfraKind::Map),
        "tuple" => Some(InfraKind::Tuple),
        "struct" => Some(InfraKind::Struct),
        _ => None,
    }
}

fn strip_article(s: &str) -> &str {
    s.strip_prefix("an ")
        .or_else(|| s.strip_prefix("a "))
        .unwrap_or(s)
}

// ── parse_type_phrase ─────────────────────────────────────────────────────────

/// Parse a type phrase (possibly with `⟦Ln⟧` placeholders) into a [`TypeExpr`].
///
/// `links[n]` gives `(display_text, TypeRef)` for placeholder `⟦Ln⟧`.
/// Used both by `declared_type` and by stage C YAML processing (which supplies
/// its own resolved link list).
pub(crate) fn parse_type_phrase(phrase: &str, links: &[(String, TypeRef)]) -> TypeExpr {
    // Normalise "-or-null" shorthand before splitting.
    let normalised = phrase.replace("-or-null", " or null");
    let parts = split_on_or(normalised.trim());
    let exprs: Vec<TypeExpr> = parts
        .into_iter()
        .map(|p| parse_single_alt(p.trim(), links))
        .collect();
    match exprs.len() {
        0 => TypeExpr::Unknown,
        1 => exprs.into_iter().next().unwrap(),
        _ => TypeExpr::Union(exprs),
    }
}

/// Split `s` on ` or ` outside `⟦...⟧` placeholders.
/// Returns individual alternative strings (owned, to avoid lifetime issues).
fn split_on_or(s: &str) -> Vec<String> {
    // A quoted enumeration like ("a" or "b") must not be split.
    if s.starts_with('(') && s.ends_with(')') {
        return vec![s.to_string()];
    }
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        // Skip over placeholder ⟦...⟧.
        if rest.starts_with('⟦') {
            if let Some(end) = rest.find('⟧') {
                i += end + '⟧'.len_utf8();
                continue;
            }
        }
        // ASCII separator " or " — safe to slice by byte index.
        if rest.starts_with(" or ") {
            parts.push(s[start..i].trim().to_string());
            i += " or ".len();
            start = i;
            continue;
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    if parts.is_empty() {
        vec![s.to_string()]
    } else {
        parts
    }
}

fn parse_single_alt(s: &str, links: &[(String, TypeRef)]) -> TypeExpr {
    let bare = strip_article(s).trim();

    // null
    if bare == "null" || s == "null" {
        return TypeExpr::Null;
    }

    // Primitive
    if let Some(p) = primitive_phrase(bare).or_else(|| primitive_phrase(s.trim())) {
        return TypeExpr::Primitive(p);
    }

    // Quoted enumeration: ("a" or "b")
    if let Some(en) = try_enumerated(s) {
        return en;
    }

    // Infra collection word with optional " of T": "list of T", "ordered set of T", …
    if let Some(infra) = try_infra_word_phrase(bare, links) {
        return infra;
    }

    // Lone placeholder ⟦Ln⟧ (possibly with leading article already stripped) → Nominal
    if let Some(nom) = try_nominal_placeholder(bare, links) {
        return nom;
    }

    // ⟦Ln⟧ where the link is an INFRA collection, optionally with " of T"
    if let Some(infra) = try_infra_link_phrase(bare, links) {
        return infra;
    }

    TypeExpr::Opaque {
        text: s.trim().to_string(),
    }
}

fn try_enumerated(s: &str) -> Option<TypeExpr> {
    static RE: OnceLock<Regex> = OnceLock::new();
    static QVAL: OnceLock<Regex> = OnceLock::new();
    if !regex(&RE, r#"^\((".+" or ".+")\)$"#).is_match(s) {
        return None;
    }
    let inner = &s[1..s.len() - 1];
    let vals: Vec<String> = regex(&QVAL, r#""([^"]+)""#)
        .captures_iter(inner)
        .map(|c| c[1].to_string())
        .collect();
    (!vals.is_empty()).then_some(TypeExpr::Enumerated(vals))
}

fn try_infra_word_phrase(bare: &str, links: &[(String, TypeRef)]) -> Option<TypeExpr> {
    // Try multi-word kinds first.
    let infra_prefixes: &[(&str, InfraKind)] = &[
        ("ordered set of ", InfraKind::OrderedSet),
        ("ordered map of ", InfraKind::OrderedMap),
        ("list of ", InfraKind::List),
        ("map of ", InfraKind::Map),
        ("tuple of ", InfraKind::Tuple),
        ("struct of ", InfraKind::Struct),
    ];
    for (prefix, kind) in infra_prefixes {
        if let Some(rest) = bare.strip_prefix(prefix) {
            let arg = parse_type_phrase(rest.trim(), links);
            return Some(TypeExpr::Infra {
                kind: *kind,
                args: vec![arg],
            });
        }
    }
    // Bare infra kind (no "of …").
    if let Some(kind) = infra_word_kind(bare) {
        return Some(TypeExpr::Infra { kind, args: vec![] });
    }
    None
}

fn try_nominal_placeholder(bare: &str, links: &[(String, TypeRef)]) -> Option<TypeExpr> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let caps = regex(&RE, r"^⟦L(\d+)⟧$").captures(bare)?;
    let idx: usize = caps[1].parse().ok()?;
    let (text, ty) = links.get(idx)?;
    // If this link is an INFRA collection kind without args, prefer Infra.
    if let Some(kind) = infra_link_kind(ty) {
        return Some(TypeExpr::Infra { kind, args: vec![] });
    }
    Some(TypeExpr::Nominal {
        ty: ty.clone(),
        text: text.clone(),
    })
}

fn try_infra_link_phrase(bare: &str, links: &[(String, TypeRef)]) -> Option<TypeExpr> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let caps = regex(&RE, r"^⟦L(\d+)⟧ of (.+)$").captures(bare)?;
    let idx: usize = caps[1].parse().ok()?;
    let (_text, ty) = links.get(idx)?;
    let kind = infra_link_kind(ty)?;
    let arg = parse_type_phrase(caps[2].trim(), links);
    Some(TypeExpr::Infra {
        kind,
        args: vec![arg],
    })
}

// ── initial_value ─────────────────────────────────────────────────────────────

/// Extract the initial value for a field declaration.
///
/// - `clause_text`: pattern text of the sentence containing the dfn.
/// - `next_sentence`: the sentence immediately following in the same block,
///   when it starts with `It is` and the block contains exactly one dfn.
/// - `intro_initial`: the word extracted from the intro paragraph
///   ("all initially unset" → `"unset"`).
pub(crate) fn initial_value(
    clause_text: &str,
    next_sentence: Option<&str>,
    intro_initial: Option<&str>,
) -> Option<InitialValue> {
    // `, set upon creation,` or `, set when …,` immediately after the dfn.
    {
        static RE: OnceLock<Regex> = OnceLock::new();
        if let Some(caps) =
            regex(&RE, r",\s*(set\s+(?:upon\s+creation|when\s+[^,]+?)),").captures(clause_text)
        {
            return Some(InitialValue::Opaque {
                text: caps[1].split_whitespace().collect::<Vec<_>>().join(" "),
            });
        }
    }

    // Patterns in clause_text that express an initial value.
    {
        static RE: OnceLock<Regex> = OnceLock::new();
        // "which must initially be V", "initially set to V", "initially V", "(default V)".
        // Order matters: longer/more-specific alternatives first.
        if let Some(caps) = regex(
            &RE,
            r"(?:which\s+must\s+initially\s+be|initially\s+set\s+to|initially|\(default)\s+([^,\.;\)⟦]+)",
        )
        .captures(clause_text)
        {
            let v = caps[1].trim().trim_end_matches(')');
            return Some(parse_value(v));
        }
    }

    // Next sentence starting with "It is …" (only when caller has already verified
    // the block has exactly one dfn).
    if let Some(sent) = next_sentence {
        static RE: OnceLock<Regex> = OnceLock::new();
        if let Some(caps) = regex(
            &RE,
            r"^It\s+is\s+(?:initially\s+)?([^,\.;\)⟦]+?)(?:\s+unless\s+otherwise\s+stated)?[,\.]",
        )
        .captures(sent.trim())
        {
            let v = caps[1].trim();
            return Some(parse_value(v));
        }
    }

    // intro_initial: a word like "unset", "null", "false".
    if let Some(word) = intro_initial {
        return Some(parse_value(word));
    }

    None
}

fn parse_value(v: &str) -> InitialValue {
    let v = v.trim();
    let text = v.to_string();
    match v {
        "true" => InitialValue::Literal {
            value: Literal::Bool(true),
            text,
        },
        "false" => InitialValue::Literal {
            value: Literal::Bool(false),
            text,
        },
        "null" => InitialValue::Literal {
            value: Literal::Null,
            text,
        },
        "undefined" => InitialValue::Literal {
            value: Literal::Undefined,
            text,
        },
        "empty" | "« »" => InitialValue::Empty { text },
        "unset" => InitialValue::Unset { text },
        _ if v.starts_with("a new ") || v.starts_with("an new ") => {
            let rest = v
                .strip_prefix("a new ")
                .or_else(|| v.strip_prefix("an new "))
                .unwrap();
            let ty = parse_type_phrase(rest, &[]);
            InitialValue::New { ty, text }
        }
        _ if is_number(v) => InitialValue::Literal {
            value: Literal::Number(v.to_string()),
            text,
        },
        _ if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 => InitialValue::Literal {
            value: Literal::String(v[1..v.len() - 1].to_string()),
            text,
        },
        _ => InitialValue::Opaque { text },
    }
}

fn is_number(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    if s.is_empty() {
        return false;
    }
    let mut has_dot = false;
    for c in s.chars() {
        if c == '.' {
            if has_dot {
                return false;
            }
            has_dot = true;
        } else if !c.is_ascii_digit() {
            return false;
        }
    }
    true
}

// ── declared_type ─────────────────────────────────────────────────────────────

/// Extract the declared type of a field from its declaration context.
///
/// `tokens` and `pat` describe the innermost block containing the dfn.
/// `dfn_slot` is the index into `pat.slots` for this field's dfn.
/// `clause` is the byte range of the sentence in `pat.text` that contains the dfn.
/// `dd_text` is the plain text of the following `<dd>` element (R4 struct items).
/// `resolve` maps an [`AnchorTarget`] to a [`TypeRef`].
pub(crate) fn declared_type(
    tokens: &[BlockToken],
    pat: &Pattern,
    dfn_slot: usize,
    clause: Range<usize>,
    dd_text: Option<&str>,
    resolve: &dyn Fn(&AnchorTarget) -> TypeRef,
) -> TypeExpr {
    let clause_text = &pat.text[clause.clone()];
    let dfn_key = format!("⟦D{dfn_slot}⟧");

    if let Some(dfn_pos_in_clause) = clause_text.find(dfn_key.as_str()) {
        let pre = &clause_text[..dfn_pos_in_clause];
        let post = &clause_text[dfn_pos_in_clause + dfn_key.len()..];

        // Pre-dfn: `a ⟦L⟧ ⟦D⟧` → Nominal (the ⟦L⟧ right before ⟦D⟧ is the type).
        if let Some(ty) = try_pre_dfn_nominal(pre, pat, tokens, resolve) {
            return ty;
        }

        // Pre-dfn: text ends with a primitive word.
        if let Some(prim) = pre_dfn_primitive(pre) {
            return TypeExpr::Primitive(prim);
        }

        // Post-dfn: type word directly after the dfn (e.g. `⟦D⟧ boolean`).
        if let Some(prim) = post_dfn_type_word(post) {
            return TypeExpr::Primitive(prim);
        }

        // Post-dfn: clause patterns.
        if let Some(ty) = extract_post_dfn_type(post, pat, tokens, resolve) {
            return ty;
        }
    }

    // dd_text fallback (R4 struct items): "A number", "An ordered set …".
    if let Some(dd) = dd_text {
        if let Some(ty) = try_dd_type(dd, pat, tokens, resolve) {
            return ty;
        }
    }

    TypeExpr::Unknown
}

/// Check for `a ⟦Ln⟧ ⟦Dm⟧`: the slot immediately before the dfn slot is a Link.
fn try_pre_dfn_nominal(
    pre: &str,
    pat: &Pattern,
    tokens: &[BlockToken],
    resolve: &dyn Fn(&AnchorTarget) -> TypeRef,
) -> Option<TypeExpr> {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Check that pre ends with `a ⟦Ln⟧` (possibly `an ⟦Ln⟧`).
    let caps = regex(&RE, r"(?:^|[ ,])a[n]?\s+⟦L(\d+)⟧\s*$").captures(pre)?;
    let slot_idx: usize = caps[1].parse().ok()?;
    let token_idx = *pat.slots.get(slot_idx)?;
    let (text, ty) = link_to_text_and_ref(&tokens[token_idx], resolve)?;
    Some(TypeExpr::Nominal { ty, text })
}

/// Text immediately before the dfn ends with a primitive type word.
fn pre_dfn_primitive(pre: &str) -> Option<Primitive> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let caps = regex(&RE, r"(?:^|[ ,])(\w+(?:\s+\w+)?)[  ]*$").captures(pre)?;
    let tail = caps[1].trim();
    // Multi-word first.
    primitive_phrase(tail)
        .or_else(|| primitive_phrase(tail.rsplit_once(' ').map(|(_, w)| w).unwrap_or(tail)))
}

/// The first word(s) of `post` are a primitive type word.
fn post_dfn_type_word(post: &str) -> Option<Primitive> {
    let s = post.trim_start_matches([' ', ',']);
    // Multi-word primitives first.
    for phrase in &[
        "scalar value string",
        "byte sequence",
        "boolean",
        "string",
        "number",
        "integer",
    ] {
        if s == *phrase
            || s.starts_with(&format!("{phrase},"))
            || s.starts_with(&format!("{phrase}."))
            || s.starts_with(&format!("{phrase} "))
        {
            return primitive_phrase(phrase);
        }
    }
    None
}

/// Try the post-dfn clause patterns (`, which is`, `, (a|an)`, etc.).
fn extract_post_dfn_type(
    post: &str,
    pat: &Pattern,
    tokens: &[BlockToken],
    resolve: &dyn Fn(&AnchorTarget) -> TypeRef,
) -> Option<TypeExpr> {
    // Build the links array for slots referenced in `post` (part of the pattern text
    // after the dfn placeholder).  We reassign consecutive 0-based indices.
    let (phrase, links) = extract_type_phrase(post, pat, tokens, resolve)?;
    Some(parse_type_phrase(&phrase, &links))
}

/// Extract the raw type phrase string and the corresponding links array from the
/// post-dfn portion of a clause.
///
/// Returns `(phrase, links)` where `phrase` uses 0-based `⟦Ln⟧` placeholders
/// that index into `links`.
fn extract_type_phrase(
    post: &str,
    pat: &Pattern,
    tokens: &[BlockToken],
    resolve: &dyn Fn(&AnchorTarget) -> TypeRef,
) -> Option<(String, Vec<(String, TypeRef)>)> {
    static R1: OnceLock<Regex> = OnceLock::new(); // , which is (an|a|either)? T
    static R2: OnceLock<Regex> = OnceLock::new(); // , (a|an) T
    static R3: OnceLock<Regex> = OnceLock::new(); // (a|an T)
    static R4: OnceLock<Regex> = OnceLock::new(); // that is (an|a)? T
    static R5: OnceLock<Regex> = OnceLock::new(); // (null or (an|a)? T)

    // Each regex captures T in group 1.
    // Articles use `(?:an|a|either)\s+` (longer alternatives first, space required)
    // so "an" is not mistakenly split into "a" + "n T".
    let patterns: &[(&OnceLock<Regex>, &str)] = &[
        (
            &R1,
            r"^,\s*which\s+is\s+(?:(?:an|a|either)\s+)?(.+?)(?:,\s*initially|[,\.;\)]|$)",
        ),
        (&R2, r"^,\s+(?:a|an)\s+(.+?)(?:,\s*initially|[,\.;\)]|$)"),
        (&R3, r"^\s*\((?:a|an)\s+(.+?)\)"),
        (
            &R4,
            r"\bthat\s+is\s+(?:(?:an|a)\s+)?(.+?)(?:,\s*initially|[,\.;\)]|$)",
        ),
        (&R5, r"^\s*\(null\s+or\s+(?:(?:an|a)\s+)?(.+?)\)"),
    ];

    for (cell, source) in patterns {
        if let Some(caps) = regex(cell, source).captures(post) {
            let raw_t = caps[1].trim();
            // For R5 "(null or T)", prepend "null or ".
            let phrase_str = if source.starts_with(r"^\s*\(null") {
                format!("null or {raw_t}")
            } else {
                raw_t.to_string()
            };
            return Some(renumber_links(&phrase_str, pat, tokens, resolve));
        }
    }
    None
}

/// Parse the dd plain text as a type when it starts with "A"/"An" (capitalised or not).
fn try_dd_type(
    dd: &str,
    pat: &Pattern,
    tokens: &[BlockToken],
    resolve: &dyn Fn(&AnchorTarget) -> TypeRef,
) -> Option<TypeExpr> {
    // Strip leading article case-insensitively ("A ", "An ", "a ", "an ").
    let trimmed = dd.trim();
    let bare = trimmed
        .strip_prefix("An ")
        .or_else(|| trimmed.strip_prefix("an "))
        .or_else(|| trimmed.strip_prefix("A "))
        .or_else(|| trimmed.strip_prefix("a "))?;
    // dd_text is plain text (no placeholders), so links = [].
    let (phrase, links) = renumber_links(bare, pat, tokens, resolve);
    Some(parse_type_phrase(&phrase, &links))
}

/// Given a raw phrase string containing `⟦Ln⟧` placeholders that refer to
/// `pat.slots[n]`, resolve each referenced link token and return a new phrase
/// string with 0-based indices alongside the corresponding links array.
fn renumber_links(
    phrase: &str,
    pat: &Pattern,
    tokens: &[BlockToken],
    resolve: &dyn Fn(&AnchorTarget) -> TypeRef,
) -> (String, Vec<(String, TypeRef)>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = regex(&RE, r"⟦L(\d+)⟧");
    let mut links: Vec<(String, TypeRef)> = Vec::new();
    let mut old_to_new: std::collections::HashMap<usize, usize> = Default::default();

    // First pass: collect and assign new indices.
    for caps in re.captures_iter(phrase) {
        let old: usize = caps[1].parse().unwrap_or(usize::MAX);
        if old_to_new.contains_key(&old) {
            continue;
        }
        if let Some(&token_idx) = pat.slots.get(old) {
            if let Some(link) = link_to_text_and_ref(&tokens[token_idx], resolve) {
                let new_idx = links.len();
                links.push(link);
                old_to_new.insert(old, new_idx);
            }
        }
    }

    // Second pass: rewrite the phrase.
    let out = re.replace_all(phrase, |caps: &regex::Captures<'_>| {
        let old: usize = caps[1].parse().unwrap_or(usize::MAX);
        match old_to_new.get(&old) {
            Some(&new_idx) => format!("⟦L{new_idx}⟧"),
            None => caps[0].to_string(), // unresolved — keep as-is
        }
    });
    (out.into_owned(), links)
}

/// Extract the display text and TypeRef from a link/dfn/code token.
fn link_to_text_and_ref(
    token: &BlockToken,
    resolve: &dyn Fn(&AnchorTarget) -> TypeRef,
) -> Option<(String, TypeRef)> {
    match token {
        BlockToken::Link {
            text,
            target: Some(target),
            ..
        } => Some((text.clone(), resolve(target))),
        BlockToken::Link {
            text, target: None, ..
        } => Some((
            text.clone(),
            TypeRef::Unresolved(AnchorTarget {
                spec: String::new(),
                anchor: String::new(),
            }),
        )),
        _ => None,
    }
}

// ── Helpers used by declare.rs ───────────────────────────────────────────────

/// Substitute `⟦Cn⟧` placeholders with their code text from `tokens`.
///
/// Call this before `initial_value` when the clause may contain quoted code
/// elements (e.g. `initially "<code>complete</code>"`).  The substitution
/// lets the initial-value regex match the whole quoted string rather than
/// stopping at the placeholder boundary.
pub(crate) fn substitute_code_tokens(s: &str, pat: &Pattern, tokens: &[BlockToken]) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    regex(&RE, r"⟦C(\d+)⟧")
        .replace_all(s, |caps: &regex::Captures<'_>| {
            let n: usize = caps[1].parse().unwrap_or(usize::MAX);
            if let Some(&ti) = pat.slots.get(n) {
                if let BlockToken::Code(text) = &tokens[ti] {
                    return text.clone();
                }
            }
            caps[0].to_string()
        })
        .into_owned()
}

// ── Helper used by declare.rs ─────────────────────────────────────────────────

/// Return the dd plain text for a `<dfn>` that sits in a `<dt>`, by finding
/// the immediately following `<dd>` sibling.  Returns `None` if the dfn is not
/// in a `<dt>` or has no following `<dd>`.
pub(crate) fn sibling_dd_text(
    dfn: &scraper::ElementRef<'_>,
    spec: &str,
    base_url: &str,
) -> Option<String> {
    use crate::state::block::{flatten, list_item, plain_text};
    let (item, _list) = list_item(dfn)?;
    if item.value().name() != "dt" {
        return None;
    }
    for sibling in item.next_siblings().filter_map(scraper::ElementRef::wrap) {
        match sibling.value().name() {
            "dd" => {
                let toks = flatten(&sibling, spec, base_url);
                return Some(plain_text(&toks));
            }
            "dt" => break, // next dt without a matching dd
            _ => {}
        }
    }
    None
}

/// Given a pattern, a dfn slot index, and the full list of sentences, return
/// `(dfn_slot_in_pat, clause_range, next_sentence_text)`.
///
/// `next_sentence_text` is `Some` only when the next sentence starts with
/// `It is` and the block contains exactly one dfn token.
pub(crate) fn locate_dfn_in_pat(
    tokens: &[BlockToken],
    pat: &Pattern,
    dfn_id: &str,
) -> Option<(usize, Range<usize>, Option<String>)> {
    // Find the slot index for this dfn.
    let dfn_slot = pat
        .slots
        .iter()
        .position(|&ti| matches!(&tokens[ti], BlockToken::Dfn { id, .. } if id == dfn_id))?;
    let placeholder = format!("⟦D{dfn_slot}⟧");
    let dfn_pos = pat.text.find(placeholder.as_str())?;
    let sents = sentences(pat);
    let clause_idx = sents.iter().position(|r| r.contains(&dfn_pos))?;
    let clause = sents[clause_idx].clone();

    // Count dfn tokens in the block.
    let dfn_count = tokens
        .iter()
        .filter(|t| matches!(t, BlockToken::Dfn { .. }))
        .count();

    let next = sents.get(clause_idx + 1).and_then(|r| {
        let t = pat.text[r.clone()].trim().to_string();
        if t.starts_with("It is") && dfn_count == 1 {
            Some(t)
        } else {
            None
        }
    });

    Some((dfn_slot, clause, next))
}
