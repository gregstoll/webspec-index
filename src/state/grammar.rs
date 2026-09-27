//! Encoder, `Parser` struct, and phrase grammar helpers for the state pass.
//!
//! The `Parser`'s statement productions live in `ir.rs` as a second `impl`
//! block on the same type. Adding a production to the grammar means adding a
//! new `impl Parser<'_>` block in its own module (e.g. `call.rs`).
use std::collections::{BTreeMap, BTreeSet};

use crate::parse::steps::{AnchorTarget, InlineTokenKind, TextSpan};
use crate::state::ir::{Hop, LinkRole, ParsedSource, Path, Root, StatementParent, StatementSource};
use crate::state::model::{Literal, TypeKey, TypeRef};

// ---------------------------------------------------------------------------
// Env and callability
// ---------------------------------------------------------------------------

/// Extraction-wide knowledge the statement grammar consults. `Default` is
/// the SP1 behavior: nothing is callable, no mentions, no body arguments.
#[derive(Debug, Clone, Default)]
pub(crate) struct Env {
    pub spec: String,
    /// In-spec anchors that can be called, by anchor.
    pub callables: BTreeMap<String, Callable>,
    /// `(segment_id, link_id)` of structural operation sites with role `Mention`.
    pub mentions: BTreeSet<(String, String)>,
    /// `(segment_id, link_id)` → body ids passed to that operation.
    pub body_args: BTreeMap<(String, String), Vec<String>>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Callable {
    Body,
    Template,
    Accessor,
    Predicate,
    NoTemplate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Callability {
    Known(Callable),
    CrossSpec,
    No,
}

impl Env {
    pub(crate) fn callability(&self, target: Option<&AnchorTarget>) -> Callability {
        match target {
            None => Callability::No,
            Some(t) if t.spec == self.spec => self
                .callables
                .get(&t.anchor)
                .copied()
                .map_or(Callability::No, Callability::Known),
            Some(_) if self.spec.is_empty() => Callability::No,
            Some(_) => Callability::CrossSpec,
        }
    }
}

/// Role strength: a stronger role replaces a weaker one, never the reverse.
pub(crate) fn role_rank(role: &LinkRole) -> u8 {
    match role {
        LinkRole::Callee { .. } => 9,
        LinkRole::ParamName { .. } => 8,
        LinkRole::Keyword => 7,
        LinkRole::InfraOp { .. } => 6,
        LinkRole::Predicate { .. } => 5,
        LinkRole::AlgorithmValue => 4,
        LinkRole::Type => 3,
        LinkRole::Field => 2,
        LinkRole::Value => 1,
        LinkRole::Unknown => 0,
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placeholder {
    /// Index into `source.links`.
    Link(usize),
    /// Index into `Encoded::vars`.
    Var(usize),
}

#[derive(Debug, Clone, Copy)]
struct Piece {
    enc: usize,
    enc_end: usize,
    src: TextSpan,
    placeholder: Option<Placeholder>,
}

/// The source text with each link span replaced by `⟦L{i}⟧` and each
/// variable outside a link by `⟦V{j}⟧`. The grammar runs on `text`; every
/// span it emits maps back through `to_src`.
pub(crate) struct Encoded {
    pub(crate) text: String,
    /// Contiguous cover of both `text` and the source text, in order.
    pieces: Vec<Piece>,
    /// Variable names (`InlineToken::source_text`).
    pub(crate) vars: Vec<String>,
    /// Encoded ranges of `Code` and `Literal` tokens.
    protected: Vec<(usize, usize)>,
    src_len: usize,
}

impl Encoded {
    pub(crate) fn new(source: &StatementSource) -> Self {
        let mut spans: Vec<(TextSpan, Placeholder)> = source
            .links
            .iter()
            .enumerate()
            .map(|(index, link)| (link.span, Placeholder::Link(index)))
            .collect();
        let mut vars = Vec::new();
        for token in &source.tokens {
            let inside_link = source
                .links
                .iter()
                .any(|link| link.span.start <= token.span.start && token.span.end <= link.span.end);
            if token.kind == InlineTokenKind::Variable && !inside_link {
                spans.push((token.span, Placeholder::Var(vars.len())));
                vars.push(token.source_text.clone());
            }
        }
        spans.sort_by_key(|(span, _)| (span.start, std::cmp::Reverse(span.end)));

        let mut encoded = Self {
            text: String::with_capacity(source.text.len()),
            pieces: Vec::new(),
            vars,
            protected: Vec::new(),
            src_len: source.text.len(),
        };
        let mut cursor = 0;
        for (span, placeholder) in spans {
            if span.start < cursor || span.start >= span.end {
                continue;
            }
            encoded.push_text(&source.text, cursor, span.start);
            let enc = encoded.text.len();
            match placeholder {
                Placeholder::Link(index) => encoded.text.push_str(&format!("⟦L{index}⟧")),
                Placeholder::Var(index) => encoded.text.push_str(&format!("⟦V{index}⟧")),
            }
            encoded.pieces.push(Piece {
                enc,
                enc_end: encoded.text.len(),
                src: span,
                placeholder: Some(placeholder),
            });
            cursor = span.end;
        }
        encoded.push_text(&source.text, cursor, source.text.len());

        encoded.protected = source
            .tokens
            .iter()
            .filter(|token| matches!(token.kind, InlineTokenKind::Code | InlineTokenKind::Literal))
            .map(|token| {
                (
                    encoded.to_enc(token.span.start),
                    encoded.to_enc(token.span.end),
                )
            })
            .collect();
        encoded
    }

    fn push_text(&mut self, src: &str, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let enc = self.text.len();
        self.text.push_str(&src[start..end]);
        self.pieces.push(Piece {
            enc,
            enc_end: self.text.len(),
            src: TextSpan { start, end },
            placeholder: None,
        });
    }

    fn piece_at(&self, enc: usize) -> Option<&Piece> {
        let index = self.pieces.partition_point(|piece| piece.enc <= enc);
        index
            .checked_sub(1)
            .map(|index| &self.pieces[index])
            .filter(|piece| enc < piece.enc_end)
    }

    /// Source byte offset of an encoded position. A placeholder's start maps
    /// to its span start, any later position inside it to its span end.
    pub(crate) fn to_src(&self, enc: usize) -> usize {
        match self.piece_at(enc) {
            None => self.src_len,
            Some(piece) if piece.placeholder.is_none() => piece.src.start + (enc - piece.enc),
            Some(piece) if enc == piece.enc => piece.src.start,
            Some(piece) => piece.src.end,
        }
    }

    /// Encoded position of a source byte offset.
    pub(crate) fn to_enc(&self, src: usize) -> usize {
        let index = self.pieces.partition_point(|piece| piece.src.start <= src);
        let Some(piece) = index.checked_sub(1).map(|index| &self.pieces[index]) else {
            return 0;
        };
        match piece.placeholder {
            _ if src >= piece.src.end => piece.enc_end,
            None => piece.enc + (src - piece.src.start),
            Some(_) if src == piece.src.start => piece.enc,
            Some(_) => piece.enc_end,
        }
    }

    pub(crate) fn span(&self, start: usize, end: usize) -> TextSpan {
        TextSpan {
            start: self.to_src(start),
            end: self.to_src(end),
        }
    }

    /// The placeholder starting exactly at `enc`, and the position after it.
    pub(crate) fn placeholder(&self, enc: usize) -> Option<(Placeholder, usize)> {
        let piece = self.piece_at(enc)?;
        (piece.enc == enc)
            .then_some(piece.placeholder)
            .flatten()
            .map(|placeholder| (placeholder, piece.enc_end))
    }

    pub(crate) fn is_protected(&self, enc: usize) -> bool {
        self.protected
            .iter()
            .any(|&(start, end)| start < enc && enc < end)
    }

    /// Link indices of the placeholders inside `start..end`.
    pub(crate) fn links_in(&self, start: usize, end: usize) -> impl Iterator<Item = usize> + '_ {
        self.pieces
            .iter()
            .filter_map(move |piece| match piece.placeholder {
                Some(Placeholder::Link(index)) if start <= piece.enc && piece.enc_end <= end => {
                    Some(index)
                }
                _ => None,
            })
    }

    /// Clause starts (§7.3 `CLAUSE`) with their lexicon verb. A step also
    /// starts one after a leading `⌛ `. Prose (§7.5) also starts a clause
    /// after its lead-ins, case-insensitively.
    pub(crate) fn clause_starts(&self, prose: bool) -> Vec<(usize, Option<String>)> {
        const PROSE_LEAD_INS: [&str; 3] = ["steps are to ", "must ", "the user agent must "];
        let bytes = self.text.as_bytes();
        let mut starts = std::collections::BTreeSet::from([0]);
        if self.text.starts_with("\u{231B} ") {
            starts.insert("\u{231B} ".len());
        }
        for (index, _) in self.text.char_indices() {
            for separator in [", ", "; ", ": "] {
                if self.text[index..].starts_with(separator) {
                    starts.insert(index + separator.len());
                }
            }
            let at_word = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
            if at_word {
                for word in ["then ", "and ", "otherwise ", "otherwise, "] {
                    if self.text[index..].starts_with(word) {
                        starts.insert(index + word.len());
                    }
                }
                if prose {
                    for lead_in in PROSE_LEAD_INS {
                        if bytes[index..]
                            .get(..lead_in.len())
                            .is_some_and(|word| word.eq_ignore_ascii_case(lead_in.as_bytes()))
                        {
                            starts.insert(index + lead_in.len());
                        }
                    }
                }
            }
        }
        starts
            .into_iter()
            .filter(|&at| at < self.text.len() && !self.is_protected(at))
            .map(|at| (at, self.verb_at(at).map(str::to_string)))
            .collect()
    }

    /// The lexicon verb spelled by the word at `enc`, if any. A hyphenated
    /// word ("set-up") is not a verb.
    pub(crate) fn verb_at(&self, enc: usize) -> Option<&'static str> {
        let word: String = self.text[enc..]
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect();
        if self.text[enc + word.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_alphanumeric() || next == '-' || next == '_')
        {
            return None;
        }
        let word = word.to_ascii_lowercase();
        crate::state::ir::LEXICON
            .iter()
            .copied()
            .find(|verb| *verb == word)
    }
}

// ---------------------------------------------------------------------------
// Path-parse result types
// ---------------------------------------------------------------------------

/// A parsed `PATH` with the link positions its statement assigns roles to.
pub(crate) struct PathParse {
    pub path: Path,
    pub end: usize,
    /// Link index of each hop; `None` for code members and slots.
    pub hop_links: Vec<Option<usize>>,
    pub root_link: Option<usize>,
    /// Encoded ranges whose links are reads (`ROOT'` phrases, subscripts).
    pub read_ranges: Vec<(usize, usize)>,
}

/// An Infra-linked mutation's target, its operand range, and the statement
/// end.
pub(crate) type InfraTarget = (PathParse, Option<(usize, usize)>, usize);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootKind {
    Var,
    This,
    /// `the`: the link followed `the `.
    Link {
        link: usize,
        the: bool,
    },
    Pronoun,
    Phrase,
}

/// A chained continuation target (`… to x and B to y`) starting at `start`.
pub(crate) struct Chained {
    pub start: usize,
    pub path: PathParse,
}

// ---------------------------------------------------------------------------
// Parser struct
// ---------------------------------------------------------------------------

pub(crate) struct Parser<'a> {
    pub(crate) enc: &'a Encoded,
    pub(crate) source: &'a StatementSource,
    pub(crate) out: ParsedSource,
    pub(crate) env: &'a Env,
    /// Clause starts that already produced an `Opaque` statement.
    pub(crate) opaque_clauses: Vec<usize>,
    /// End of the last structured statement; clauses before it are inside it.
    pub(crate) covered_until: usize,
    /// Encoded position of each initializer's `a new` → its `Init` id.
    pub(crate) inits: BTreeMap<usize, String>,
    /// The inline block the next pushed statement belongs to (§8.2).
    pub(crate) inline_parent: Option<StatementParent>,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(enc: &'a Encoded, source: &'a StatementSource, env: &'a Env) -> Self {
        Self {
            enc,
            source,
            out: ParsedSource::default(),
            env,
            opaque_clauses: Vec::new(),
            covered_until: 0,
            inits: BTreeMap::new(),
            inline_parent: None,
        }
    }

    pub(crate) fn set_role(&mut self, link: usize, role: LinkRole) {
        let keep = self
            .out
            .roles
            .get(&link)
            .is_some_and(|old| role_rank(old) >= role_rank(&role));
        if !keep {
            self.out.roles.insert(link, role);
        }
    }
}

// ---------------------------------------------------------------------------
// Grammar helpers
// ---------------------------------------------------------------------------

/// Grammar helpers over the encoded text.
impl Parser<'_> {
    pub(crate) fn lit(&self, pos: usize, s: &str) -> bool {
        self.enc.text[pos..].starts_with(s)
    }

    /// The position after the first of `words` that starts at `pos`.
    pub(crate) fn keyword(&self, pos: usize, words: &[&str]) -> Option<usize> {
        words
            .iter()
            .find(|word| self.lit(pos, word))
            .map(|word| pos + word.len())
    }

    pub(crate) fn find(&self, pos: usize, s: &str) -> Option<usize> {
        self.enc.text[pos..].find(s).map(|found| pos + found)
    }

    /// Canonical source text of an encoded range.
    pub(crate) fn src_text(&self, start: usize, end: usize) -> String {
        let span = self.enc.span(start, end);
        self.source.text[span.start..span.end].to_string()
    }

    /// Encoded range rendered with every link replaced by `_`.
    pub(crate) fn render(&self, start: usize, end: usize) -> String {
        let mut out = String::new();
        let mut pos = start;
        while pos < end {
            match self.enc.placeholder(pos) {
                Some((Placeholder::Link(_), after)) => {
                    out.push('_');
                    pos = after;
                }
                Some((Placeholder::Var(_), after)) => {
                    out.push_str(&self.src_text(pos, after));
                    pos = after;
                }
                None => {
                    let ch = self.enc.text[pos..].chars().next().expect("in bounds");
                    out.push(ch);
                    pos += ch.len_utf8();
                }
            }
        }
        out.trim().to_string()
    }

    /// `END`: `.`, `,`, `;`, ` and `, ` if `, ` otherwise`, or end of text.
    pub(crate) fn is_end(&self, pos: usize) -> bool {
        pos == self.enc.text.len()
            || [".", ",", ";", " and ", " if ", " otherwise"]
                .iter()
                .any(|end| self.lit(pos, end))
    }

    pub(crate) fn is_this_link(&self, link: usize) -> bool {
        self.source.links[link]
            .target
            .as_ref()
            .is_some_and(|target| {
                target.spec.eq_ignore_ascii_case("WEBIDL") && target.anchor == "this"
            })
    }

    pub(crate) fn field_hop(&self, link: usize) -> Hop {
        let link = &self.source.links[link];
        Hop::Field {
            link_id: link.id.clone(),
            target: link.target.clone(),
            visible_text: link.visible_text.clone(),
        }
    }

    /// `TARGETS`: a `PATH`, then `(", and " | ", " | " and ") ⟦L⟧ TRAILER?`
    /// continuations sharing its root and prefix hops (an implicit root
    /// also takes `the ⟦L⟧`). The whole list must end where `accept` holds.
    pub(crate) fn targets(
        &self,
        pos: usize,
        to_terminated: bool,
        accept: impl Fn(&Self, usize) -> bool,
    ) -> Option<(Vec<PathParse>, usize)> {
        let first = self.path(pos, to_terminated)?;
        let mut end = first.end;
        let mut more = Vec::new();
        if !first.path.hops.is_empty() && first.path.subscript.is_none() {
            while let Some(mut next) = self.keyword(end, &[", and ", ", ", " and "]) {
                if first.path.root == Root::Implicit {
                    next = self.keyword(next, &["the "]).unwrap_or(next);
                }
                // `this's ⟦A⟧ and this's ⟦B⟧`: a full path repeating the root.
                if matches!(first.path.root, Root::This | Root::Var(_)) {
                    if let Some(path) = self.path(next, to_terminated).filter(|p| {
                        p.path.root == first.path.root
                            && !p.path.hops.is_empty()
                            && p.path.subscript.is_none()
                    }) {
                        end = path.end;
                        more.push(path);
                        continue;
                    }
                }
                let Some((hop, Some(link), after)) = self.hop(next) else {
                    break;
                };
                end = self.trailer(after);
                more.push(self.with_last_hop(&first, hop, link, end));
            }
        }
        if !accept(self, end) {
            return None;
        }
        let mut targets = vec![first];
        targets.append(&mut more);
        Some((targets, end))
    }

    /// `basis` with its last hop replaced by the linked `hop`: the same root
    /// and prefix hops.
    pub(crate) fn with_last_hop(
        &self,
        basis: &PathParse,
        hop: Hop,
        link: usize,
        end: usize,
    ) -> PathParse {
        let mut path = basis.path.clone();
        path.hops.pop();
        path.hops.push(hop);
        let mut hop_links = basis.hop_links.clone();
        hop_links.pop();
        hop_links.push(Some(link));
        PathParse {
            path,
            end,
            hop_links,
            root_link: basis.root_link,
            read_ranges: Vec::new(),
        }
    }

    /// `PATH`. With `to_terminated`, also `the ⟦L⟧ of ROOT'` where `ROOT'`
    /// runs up to ` to `.
    pub(crate) fn path(&self, pos: usize, to_terminated: bool) -> Option<PathParse> {
        if to_terminated {
            if let Some(path) = self.of_path(pos) {
                return Some(path);
            }
        }
        let (mut root, kind, mut at) = self.root(pos)?;
        let mut read_ranges = Vec::new();
        if kind == RootKind::Phrase {
            read_ranges.push((pos, at));
        }
        let mut hops = Vec::new();
        let mut hop_links = Vec::new();
        if kind == RootKind::Pronoun {
            let (hop, link, end) = self.hop(at)?;
            hops.push(hop);
            hop_links.push(link);
            at = end;
        }
        loop {
            if let Some(after) = self.keyword(at, &["'s ", "\u{2019}s "]) {
                let Some((hop, link, end)) = self.hop(after) else {
                    break;
                };
                hops.push(hop);
                hop_links.push(link);
                at = end;
            } else if let Some((name, end)) = self.slot(at) {
                hops.push(Hop::Slot { name });
                hop_links.push(None);
                at = end;
            } else {
                break;
            }
        }
        let mut root_link = None;
        if let RootKind::Link { link, the } = kind {
            // Only `the ⟦L⟧` is receiver-less (§7.3); a bare link without
            // hops stays a `Root::Link` path with no field hop.
            if hops.is_empty() && the {
                root = Root::Implicit;
                hops.push(self.field_hop(link));
                hop_links.push(Some(link));
                if self.source.links[link].visible_text.starts_with('`') {
                    at = self.attribute_suffix(at).unwrap_or(at);
                }
            } else {
                root_link = Some(link);
            }
        }
        if kind == RootKind::Phrase && hops.is_empty() {
            return None;
        }
        if !hops.is_empty() {
            at = self.trailer(at);
        }
        let mut subscript = None;
        if self.lit(at, "[") {
            let close = self.matching_bracket(at)?;
            subscript = Some(Box::new(self.value_expr(at + 1, close, &mut Vec::new())));
            read_ranges.push((at + 1, close));
            at = close + 1;
        }
        Some(PathParse {
            path: Path {
                root,
                hops,
                subscript,
            },
            end: at,
            hop_links,
            root_link,
            read_ranges,
        })
    }

    pub(crate) fn of_path(&self, pos: usize) -> Option<PathParse> {
        let the = self.keyword(pos, &["the "]).unwrap_or(pos);
        let (Placeholder::Link(link), after) = self.enc.placeholder(the)? else {
            return None;
        };
        let root_start = self.keyword(after, &[" of "])?;
        // `ROOT'` stays inside the clause: it ends at ` to ` before the value
        // end and never spans `, `.
        let root_end = self
            .find(root_start, " to ")
            .filter(|&to| to < self.value_end(root_start, None))?;
        if self.enc.text[root_start..root_end].contains(", ") {
            return None;
        }
        let root = match self.root(root_start) {
            Some((root, RootKind::Var | RootKind::This, end)) if end == root_end => root,
            Some((root @ Root::Link { .. }, RootKind::Link { .. }, end)) if end == root_end => root,
            _ => Root::Opaque {
                text: self.src_text(root_start, root_end),
            },
        };
        Some(PathParse {
            path: Path {
                root,
                hops: vec![self.field_hop(link)],
                subscript: None,
            },
            end: root_end,
            hop_links: vec![Some(link)],
            root_link: None,
            read_ranges: vec![(root_start, root_end)],
        })
    }

    /// `ROOT`: `⟦V⟧`, `this`, the`? `⟦L⟧`, a pronoun, or a 1–6 word phrase
    /// followed by `POSS HOP`.
    pub(crate) fn root(&self, pos: usize) -> Option<(Root, RootKind, usize)> {
        if let Some(end) = self.keyword(pos, &["its ", "their "]) {
            let text = self.enc.text[pos..end - 1].to_string();
            return Some((Root::Opaque { text }, RootKind::Pronoun, end));
        }
        let direct = self.direct_root(pos);
        if let Some((_, _, end)) = direct {
            if self.keyword(end, &["'s ", "\u{2019}s ", ".[["]).is_some() {
                return direct;
            }
        }
        // "this element's F", "the ⟦`img`⟧ element's F"
        self.phrase_root(pos).or(direct)
    }

    /// `⟦V⟧`, `this`, or `the`? `⟦L⟧` / `the ⟦V⟧`.
    pub(crate) fn direct_root(&self, pos: usize) -> Option<(Root, RootKind, usize)> {
        let at = self.keyword(pos, &["the "]).unwrap_or(pos);
        match self.enc.placeholder(at) {
            Some((Placeholder::Var(var), end)) => {
                Some((Root::Var(self.enc.vars[var].clone()), RootKind::Var, end))
            }
            Some((Placeholder::Link(link), end)) => Some(self.link_root(link, end, at > pos)),
            None if at == pos && self.lit(pos, "this") && !self.word_continues(pos + 4) => {
                Some((Root::This, RootKind::This, pos + 4))
            }
            None => None,
        }
    }

    pub(crate) fn link_root(&self, link: usize, end: usize, the: bool) -> (Root, RootKind, usize) {
        if self.is_this_link(link) {
            return (Root::This, RootKind::This, end);
        }
        let root = Root::Link {
            link_id: self.source.links[link].id.clone(),
            target: self.source.links[link].target.clone(),
        };
        (root, RootKind::Link { link, the }, end)
    }

    pub(crate) fn phrase_root(&self, pos: usize) -> Option<(Root, RootKind, usize)> {
        let mut at = self.keyword(pos, &["the "]).unwrap_or(pos);
        // "set the element's F" is a clause, not a phrase "set the element".
        if at == pos && self.enc.verb_at(pos).is_some() {
            return None;
        }
        for _ in 0..6 {
            let word_end = self.word_end(at)?;
            if let Some(after) = self.keyword(word_end, &["'s ", "\u{2019}s "]) {
                self.hop(after)?;
                let text = self.src_text(pos, word_end);
                return Some((Root::Opaque { text }, RootKind::Phrase, word_end));
            }
            at = self.keyword(word_end, &[" "])?;
        }
        None
    }

    /// End of a phrase word at `pos`: `[A-Za-z][\w-]*` other than a word
    /// that ends a noun phrase, a code token, or a link.
    pub(crate) fn word_end(&self, pos: usize) -> Option<usize> {
        const STOP_WORDS: [&str; 14] = [
            "to", "and", "or", "if", "then", "be", "is", "are", "by", "with", "from", "into", "as",
            "for",
        ];
        if let Some((Placeholder::Link(_), end)) = self.enc.placeholder(pos) {
            return Some(end);
        }
        if let Some((_, end)) = self.code(pos) {
            return Some(end);
        }
        let bytes = self.enc.text.as_bytes();
        if !bytes.get(pos)?.is_ascii_alphabetic() {
            return None;
        }
        let mut end = pos + 1;
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'_' | b'-'))
        {
            end += 1;
        }
        (!STOP_WORDS.contains(&&self.enc.text[pos..end])).then_some(end)
    }

    pub(crate) fn word_continues(&self, pos: usize) -> bool {
        self.enc
            .text
            .as_bytes()
            .get(pos)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    }

    /// `HOP`: `⟦L⟧`, or `` `name` attribute`` with an optional ` value`. A
    /// linked code member (`` ⟦`name`⟧ attribute``) is a field hop.
    pub(crate) fn hop(&self, pos: usize) -> Option<(Hop, Option<usize>, usize)> {
        if let Some((Placeholder::Link(link), end)) = self.enc.placeholder(pos) {
            let end = if self.source.links[link].visible_text.starts_with('`') {
                self.attribute_suffix(end).unwrap_or(end)
            } else {
                end
            };
            return Some((self.field_hop(link), Some(link), end));
        }
        let (name, end) = self.code(pos)?;
        let end = self.attribute_suffix(end)?;
        Some((Hop::CodeMember { name }, None, end))
    }

    /// ` attribute(s)` or ` IDL attribute(s)`, with an optional ` value`.
    pub(crate) fn attribute_suffix(&self, pos: usize) -> Option<usize> {
        let end = [
            " attributes",
            " attribute",
            " IDL attributes",
            " IDL attribute",
        ]
        .iter()
        .find(|word| self.lit(pos, word) && !self.word_continues(pos + word.len()))
        .map(|word| pos + word.len())?;
        Some(
            self.keyword(end, &[" value"])
                .filter(|&value| !self.word_continues(value))
                .unwrap_or(end),
        )
    }

    /// A backtick code token at `pos`, unescaped, and the position after it.
    pub(crate) fn code(&self, pos: usize) -> Option<(String, usize)> {
        if !self.lit(pos, "`") {
            return None;
        }
        let mut name = String::new();
        let mut chars = self.enc.text[pos + 1..].char_indices();
        while let Some((offset, ch)) = chars.next() {
            match ch {
                '\\' => name.push(chars.next()?.1),
                '`' => return Some((name, pos + 1 + offset + 1)),
                _ => name.push(ch),
            }
        }
        None
    }

    /// `.[[Name]]` directly after a root or hop; ecmarkup writes the slot as a
    /// variable (`.⟦V⟧` spelling `[[Name]]`).
    pub(crate) fn slot(&self, pos: usize) -> Option<(String, usize)> {
        let dot = self.keyword(pos, &["."])?;
        let (name, end) = match self.enc.placeholder(dot) {
            Some((Placeholder::Var(var), end)) => {
                let name = self.enc.vars[var].strip_prefix("[[")?.strip_suffix("]]")?;
                (name, end)
            }
            _ => {
                let start = self.keyword(dot, &["[["])?;
                let close = self.find(start, "]]")?;
                (&self.enc.text[start..close], close + 2)
            }
        };
        (!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .then(|| (name.to_string(), end))
    }

    /// `TRAILER`: ` flag`, ` flags`, ` state` or ` boolean` as a whole word.
    pub(crate) fn trailer(&self, pos: usize) -> usize {
        [" flags", " flag", " state", " boolean"]
            .iter()
            .find(|word| self.lit(pos, word) && !self.word_continues(pos + word.len()))
            .map_or(pos, |word| pos + word.len())
    }

    pub(crate) fn matching_bracket(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (offset, byte) in self.enc.text.as_bytes()[open..].iter().enumerate() {
            match byte {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(open + offset);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// End of a `VALUE` starting at `start`. With `chain` (the target of the
    /// statement being parsed), a chained `Set` continuation also ends it.
    pub(crate) fn value_end(&self, start: usize, chain: Option<&PathParse>) -> usize {
        let text = &self.enc.text;
        for (offset, _) in text[start..].char_indices() {
            let i = start + offset;
            if self.enc.is_protected(i) {
                continue;
            }
            let rest = &text[i..];
            if rest.starts_with(". ")
                || rest == "."
                || rest.starts_with(';')
                || rest.starts_with(", then ")
                || rest.starts_with(", and then ")
                || rest.starts_with(", otherwise")
            {
                return i;
            }
            if let Some(after) = self.keyword(i, &[", and ", ", ", " and "]) {
                if self.verb_follows(after) {
                    return i;
                }
            }
            if chain.is_some_and(|basis| self.chained_continuation(i, basis).is_some()) {
                return i;
            }
        }
        text.len()
    }

    /// End of the `VALUE` of a `Let`, `Set`, `Return` or passive set, which
    /// may be a `COND`: past a top-level `; otherwise(,)? B` when the value
    /// before it has a top-level ` if ` and `B` starts with no verb (a
    /// clause of its own: "…; otherwise return null").
    pub(crate) fn statement_value_end(&self, start: usize, chain: Option<&PathParse>) -> usize {
        let end = self.value_end(start, chain);
        let Some(after) = self.keyword(end, &["; otherwise"]) else {
            return end;
        };
        let otherwise_start = self.keyword(after, &[", ", " "]).unwrap_or(after);
        if self.word_continues(after)
            || self.verb_follows(otherwise_start)
            || (self.enc.placeholder(otherwise_start).is_none()
                && self.is_verb_head(otherwise_start))
            || !self
                .top_level(start, end)
                .into_iter()
                .any(|pos| pos > start && self.lit(pos, " if "))
        {
            return end;
        }
        self.value_end(otherwise_start, chain)
    }

    /// Starts of the sentences after the first: after a top-level `. `, at a
    /// capital letter or a link whose text starts with one.
    pub(crate) fn sentence_starts(&self) -> Vec<usize> {
        self.top_level(0, self.enc.text.len())
            .into_iter()
            .filter_map(|pos| self.keyword(pos, &[". "]))
            .filter(|&start| match self.enc.placeholder(start) {
                Some((Placeholder::Link(link), _)) => self.source.links[link]
                    .visible_text
                    .starts_with(char::is_uppercase),
                Some(_) => false,
                None => self.enc.text[start..].starts_with(char::is_uppercase),
            })
            .collect()
    }

    /// A lexicon verb or an Infra operation link, then a space, at `pos`.
    pub(crate) fn verb_follows(&self, pos: usize) -> bool {
        if let Some((_, after_link, ..)) = self.infra_op(pos) {
            return self.lit(after_link, " ");
        }
        self.enc
            .verb_at(pos)
            .is_some_and(|verb| self.lit(pos + verb.len(), " "))
    }

    /// The target of a chained continuation at `pos`, followed by ` to `:
    /// - a `PATH` with hops or a variable root that doesn't span `, `;
    /// - a bare `⟦L⟧ TRAILER?`, which shares the receiver of `basis` (the
    ///   previous target), as in "Set R's A to x, B to y". It needs a
    ///   receiver: after a receiver-less or hop-less target, a bare link is
    ///   prose ("…, seek to that time", "*x* to *y*, clamped to the range").
    pub(crate) fn chained_path(&self, pos: usize, basis: &PathParse) -> Option<PathParse> {
        if let Some((hop, Some(link), after)) = self.hop(pos) {
            let end = self.trailer(after);
            if self.lit(end, " to ") {
                let has_receiver = basis.path.root != Root::Implicit
                    && !basis.path.hops.is_empty()
                    && basis.path.subscript.is_none();
                return has_receiver.then(|| self.with_last_hop(basis, hop, link, end));
            }
        }
        self.path(pos, true).filter(|path| {
            self.lit(path.end, " to ")
                && (!path.path.hops.is_empty() || matches!(path.path.root, Root::Var(_)))
                && !self.enc.text[pos..path.end].contains(", ")
        })
    }

    /// `(", and " | ", " | " and ")` and a chained target at `end`.
    pub(crate) fn chained_continuation(&self, end: usize, basis: &PathParse) -> Option<Chained> {
        let start = self.keyword(end, &[", and ", ", ", " and "])?;
        let path = self.chained_path(start, basis)?;
        Some(Chained { start, path })
    }

    /// A value that starts with `be ` and a link, or with a link whose text
    /// starts with `be `, is an invocation.
    pub(crate) fn is_invocation(&self, pos: usize) -> bool {
        if let Some(after) = self.keyword(pos, &["be "]) {
            if matches!(self.enc.placeholder(after), Some((Placeholder::Link(_), _))) {
                return true;
            }
        }
        matches!(self.enc.placeholder(pos), Some((Placeholder::Link(link), _))
            if self.source.links[link].visible_text.starts_with("be "))
    }

    /// `NEWTYPE`: a link, or a code token naming an IDL interface.
    pub(crate) fn new_type(&self, pos: usize) -> Option<TypeRef> {
        if let Some((Placeholder::Link(link), _)) = self.enc.placeholder(pos) {
            return self.source.links[link]
                .target
                .clone()
                .map(TypeRef::Unresolved);
        }
        let (name, _) = self.code(pos)?;
        let mut chars = name.chars();
        let is_identifier = chars
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        is_identifier.then_some(TypeRef::Known(TypeKey::Idl(name)))
    }
}

// ---------------------------------------------------------------------------
// Literal helper
// ---------------------------------------------------------------------------

/// `true`/`false`/`null`/`undefined`, a number, or a quoted string.
pub(crate) fn literal(text: &str) -> Option<Literal> {
    match text {
        "true" => return Some(Literal::Bool(true)),
        "false" => return Some(Literal::Bool(false)),
        "null" => return Some(Literal::Null),
        "undefined" => return Some(Literal::Undefined),
        _ => {}
    }
    let digits = text.strip_prefix('-').unwrap_or(text);
    if !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && digits.bytes().next().is_some_and(|b| b.is_ascii_digit())
        && digits.bytes().filter(|b| *b == b'.').count() <= 1
    {
        return Some(Literal::Number(text.to_string()));
    }
    for (open, close) in [('"', '"'), ('\u{201C}', '\u{201D}')] {
        if let Some(inner) = text.strip_prefix(open).and_then(|t| t.strip_suffix(close)) {
            // A quoted code token ("`html`") is the string it spells.
            let inner = inner
                .strip_prefix('`')
                .and_then(|t| t.strip_suffix('`'))
                .filter(|t| !t.contains('`'))
                .unwrap_or(inner);
            return Some(Literal::String(inner.to_string()));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ir::tests_support::sources;
    use crate::state::ir::LinkRole;

    #[test]
    fn callability_distinguishes_in_spec_cross_spec_and_unknown() {
        let mut env = Env {
            spec: "HTML".into(),
            ..Env::default()
        };
        env.callables.insert("navigate".into(), Callable::Template);
        let t = |spec: &str, anchor: &str| AnchorTarget {
            spec: spec.into(),
            anchor: anchor.into(),
        };
        assert_eq!(
            env.callability(Some(&t("HTML", "navigate"))),
            Callability::Known(Callable::Template)
        );
        assert_eq!(
            env.callability(Some(&t("HTML", "navigable"))),
            Callability::No
        );
        assert_eq!(
            env.callability(Some(&t("DOM", "concept-node-insert"))),
            Callability::CrossSpec
        );
        assert_eq!(env.callability(None), Callability::No);
        assert_eq!(
            Env::default().callability(Some(&t("DOM", "x"))),
            Callability::No
        );
    }

    #[test]
    fn stronger_roles_win_regardless_of_order() {
        let src = sources(&[r##"Set <var>x</var> to <a href="#y">y</a>."##]).remove(0);
        let enc = Encoded::new(&src);
        let env = Env::default();
        let mut p = Parser::new(&enc, &src, &env);
        p.set_role(0, LinkRole::Value);
        p.set_role(0, LinkRole::Callee { call: "c".into() });
        p.set_role(0, LinkRole::Field);
        assert_eq!(p.out.roles[&0], LinkRole::Callee { call: "c".into() });
    }
}
