//! Minimal statement IR (§7.1 sources, §7.2 statements and expressions) and
//! its grammar (§7.3).
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::parse::steps::{AnchorTarget, InlineToken, InlineTokenKind, LinkSpan, TextSpan};
use crate::state::model::{Literal, OccurrenceClass, TypeKey, TypeRef};

// ---------------------------------------------------------------------------
// §7.1 Sources
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementSource {
    pub id: String,
    pub subject: AnchorTarget,
    pub context: SourceContext,
    pub text: String,
    pub tokens: Vec<InlineToken>,
    pub links: Vec<LinkSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceContext {
    Algorithm {
        segment_id: String,
        step_id: Option<String>,
        step_path: Option<String>,
        body_id: String,
    },
    BranchLabel {
        branch_id: String,
        step_id: String,
        step_path: String,
        segment_id: String,
    },
    Prose {
        node_id: String,
        step_path: Option<String>,
        role: ProseRole,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProseRole {
    Steps,
    Getter,
    Setter,
    Method,
    Constructor,
    Normative,
}

// ---------------------------------------------------------------------------
// §7.2 Statements, expressions, paths
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub id: String,
    pub source_id: String,
    pub span: TextSpan,
    pub kind: StatementKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementKind {
    Let {
        var: String,
        value: Expr,
    },
    Set {
        targets: Vec<Path>,
        value: Expr,
        form: SetForm,
    },
    Mutate {
        op: MutationOp,
        target: Path,
        operand: Option<Expr>,
        basis: OpBasis,
    },
    Init {
        constructed: Option<TypeRef>,
        entries: Vec<InitEntry>,
        form: InitForm,
    },
    /// `verb`: the clause-initial lexicon verb (§7.4), when there is one.
    /// `target_text`: the text after the verb up to ` to ` or the statement
    /// end, with linked spans elided; the field query matches it for possible
    /// unlinked writes.
    Opaque {
        reason: OpaqueReason,
        verb: Option<String>,
        target_text: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetForm {
    To,
    Flag,
    Passive,
    Chained,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOp {
    Append,
    Prepend,
    Extend,
    Insert,
    Remove,
    Replace,
    Empty,
    Clear,
    MapSet,
    MapRemove,
    Enqueue,
    Dequeue,
    Increment,
    Decrement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpBasis {
    InfraLink(AnchorTarget),
    Verb,
    Rule { rule_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitEntry {
    pub field: Hop,
    pub value: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitForm {
    WhoseList,
    WithItsSetTo,
    WithList,
    DlEntries,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expr {
    Var(String),
    This,
    Literal(Literal),
    Path(Path),
    New {
        ty: Option<TypeRef>,
        init: Option<String>,
    },
    Opaque {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Path {
    pub root: Root,
    pub hops: Vec<Hop>,
    pub subscript: Option<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Root {
    Var(String),
    This,
    Link {
        link_id: String,
        target: Option<AnchorTarget>,
    },
    Implicit,
    Opaque {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hop {
    Field {
        link_id: String,
        target: Option<AnchorTarget>,
        visible_text: String,
    },
    CodeMember {
        name: String,
    },
    Slot {
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpaqueReason {
    UnparsedTarget,
    PronounRoot,
    ValueIsInvocation,
    UnsupportedForm,
    Other,
}

// ---------------------------------------------------------------------------
// §7.3 Grammar
// ---------------------------------------------------------------------------

/// Clause-initial mutation verbs (§7.4). They decide between `read` and
/// `unclassified`; they never produce a write.
#[allow(dead_code)]
pub(crate) const LEXICON: [&str; 21] = [
    "set",
    "unset",
    "append",
    "prepend",
    "remove",
    "insert",
    "extend",
    "replace",
    "empty",
    "clear",
    "increment",
    "decrement",
    "update",
    "change",
    "reset",
    "enqueue",
    "dequeue",
    "add",
    "store",
    "mark",
    "toggle",
];

#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedSource {
    pub statements: Vec<Statement>,
    /// link index (into `source.links`) → (class, statement id) for every
    /// link a statement positions.
    pub link_roles: BTreeMap<usize, (OccurrenceClass, String)>,
    pub clauses: Vec<Clause>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Clause {
    /// Byte offset into the source text.
    pub start: usize,
    /// The clause-initial lexicon verb, lowercase.
    pub verb: Option<String>,
    pub consumed: bool,
}

/// `stmt-` + sha256(source_id \0 kind \0 start \0 end).
#[allow(dead_code)]
pub(crate) fn statement_id(source_id: &str, kind: &str, span: TextSpan) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_id.as_bytes());
    for component in [kind, &span.start.to_string(), &span.end.to_string()] {
        hasher.update([0]);
        hasher.update(component.as_bytes());
    }
    format!("stmt-{:x}", hasher.finalize())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placeholder {
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
struct Encoded {
    text: String,
    /// Contiguous cover of both `text` and the source text, in order.
    pieces: Vec<Piece>,
    /// Variable names (`InlineToken::source_text`).
    vars: Vec<String>,
    /// Encoded ranges of `Code` and `Literal` tokens.
    protected: Vec<(usize, usize)>,
    src_len: usize,
}

impl Encoded {
    fn new(source: &StatementSource) -> Self {
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
    fn to_src(&self, enc: usize) -> usize {
        match self.piece_at(enc) {
            None => self.src_len,
            Some(piece) if piece.placeholder.is_none() => piece.src.start + (enc - piece.enc),
            Some(piece) if enc == piece.enc => piece.src.start,
            Some(piece) => piece.src.end,
        }
    }

    /// Encoded position of a source byte offset.
    fn to_enc(&self, src: usize) -> usize {
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

    fn span(&self, start: usize, end: usize) -> TextSpan {
        TextSpan {
            start: self.to_src(start),
            end: self.to_src(end),
        }
    }

    /// The placeholder starting exactly at `enc`, and the position after it.
    fn placeholder(&self, enc: usize) -> Option<(Placeholder, usize)> {
        let piece = self.piece_at(enc)?;
        (piece.enc == enc)
            .then_some(piece.placeholder)
            .flatten()
            .map(|placeholder| (placeholder, piece.enc_end))
    }

    fn is_protected(&self, enc: usize) -> bool {
        self.protected
            .iter()
            .any(|&(start, end)| start < enc && enc < end)
    }

    /// Link indices of the placeholders inside `start..end`.
    fn links_in(&self, start: usize, end: usize) -> impl Iterator<Item = usize> + '_ {
        self.pieces
            .iter()
            .filter_map(move |piece| match piece.placeholder {
                Some(Placeholder::Link(index)) if start <= piece.enc && piece.enc_end <= end => {
                    Some(index)
                }
                _ => None,
            })
    }

    /// Clause starts (§7.3 `CLAUSE`) with their lexicon verb.
    fn clause_starts(&self) -> Vec<(usize, Option<String>)> {
        let bytes = self.text.as_bytes();
        let mut starts = std::collections::BTreeSet::from([0]);
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
            }
        }
        starts
            .into_iter()
            .filter(|&at| at < self.text.len() && !self.is_protected(at))
            .map(|at| (at, self.verb_at(at).map(str::to_string)))
            .collect()
    }

    /// The lexicon verb spelled by the word at `enc`, if any.
    fn verb_at(&self, enc: usize) -> Option<&'static str> {
        let word: String = self.text[enc..]
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect();
        let word = word.to_ascii_lowercase();
        LEXICON.iter().copied().find(|verb| *verb == word)
    }
}

/// Parse one statement source (§7.3). Every clause gets a `Clause`; each
/// clause that starts with a lexicon verb and yields no structured statement
/// gets an `Opaque` statement.
#[allow(dead_code)]
pub(crate) fn parse_source(source: &StatementSource) -> ParsedSource {
    let enc = Encoded::new(source);
    let mut p = Parser {
        enc: &enc,
        source,
        out: ParsedSource::default(),
        opaque_clauses: Vec::new(),
        covered_until: 0,
    };
    for (at, verb) in enc.clause_starts() {
        let consumed = at < p.covered_until
            || p.try_set(at)
            || p.try_unset(at)
            || p.try_incdec(at)
            || p.try_let(at)
            || p.try_passive(at);
        p.out.clauses.push(Clause {
            start: enc.to_src(at),
            verb: verb.clone(),
            consumed,
        });
        if !consumed && verb.is_some() && !p.opaque_clauses.contains(&at) {
            p.push_opaque(at, OpaqueReason::UnsupportedForm, verb);
        }
    }
    p.out
}

/// A parsed `PATH` with the link positions its statement assigns roles to.
struct PathParse {
    path: Path,
    end: usize,
    /// Link index of each hop; `None` for code members and slots.
    hop_links: Vec<Option<usize>>,
    root_link: Option<usize>,
    /// Encoded ranges whose links are reads (`ROOT'` phrases, subscripts).
    read_ranges: Vec<(usize, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RootKind {
    Var,
    This,
    Link(usize),
    Pronoun,
    Phrase,
}

struct Parser<'a> {
    enc: &'a Encoded,
    source: &'a StatementSource,
    out: ParsedSource,
    /// Clause starts that already produced an `Opaque` statement.
    opaque_clauses: Vec<usize>,
    /// End of the last structured statement; clauses before it are inside it.
    covered_until: usize,
}

impl Parser<'_> {
    fn try_set(&mut self, at: usize) -> bool {
        let Some(after) = self.keyword(at, &["Set ", "set "]) else {
            return false;
        };
        if let Some((targets, end)) = self.targets(after, true, |p, end| p.lit(end, " to ")) {
            let value_start = end + " to ".len();
            if self.is_invocation(value_start) {
                let id = self.push_opaque(at, OpaqueReason::ValueIsInvocation, Some("set".into()));
                for target in &targets {
                    for link in target
                        .root_link
                        .iter()
                        .chain(target.hop_links.iter().flatten())
                    {
                        self.role(*link, OccurrenceClass::ReadPath, &id);
                    }
                }
                return false;
            }
            let mut end = self.push_assignment(at, &targets, value_start, SetForm::To);
            while let Some(chained) = self.chained_continuation(end) {
                let value_start = chained.path.end + " to ".len();
                let targets = std::slice::from_ref(&chained.path);
                end = self.push_assignment(chained.start, targets, value_start, SetForm::Chained);
            }
            self.covered_until = end;
            return true;
        }
        if self.try_flag(after, at, true) {
            return true;
        }
        let reason = if self.lit(after, "it to ") || self.lit(after, "them to ") {
            OpaqueReason::PronounRoot
        } else {
            OpaqueReason::UnparsedTarget
        };
        self.push_opaque(at, reason, Some("set".into()));
        false
    }

    /// `Set TARGETS to VALUE` from `start`, or `Mutate { MapSet }` for a
    /// subscripted target. Returns the value end.
    fn push_assignment(
        &mut self,
        start: usize,
        targets: &[PathParse],
        value_start: usize,
        form: SetForm,
    ) -> usize {
        let value_end = self.value_end(value_start, true);
        let value = self.expr(value_start, value_end);
        let id = if targets[0].path.subscript.is_some() {
            let kind = StatementKind::Mutate {
                op: MutationOp::MapSet,
                target: targets[0].path.clone(),
                operand: Some(value),
                basis: OpBasis::Verb,
            };
            self.push(start, value_end, "mutate", kind)
        } else {
            let kind = StatementKind::Set {
                targets: targets.iter().map(|t| t.path.clone()).collect(),
                value,
                form,
            };
            self.push(start, value_end, "set", kind)
        };
        self.target_roles(targets, &id);
        self.read_roles(value_start, value_end, &id);
        value_end
    }

    fn try_unset(&mut self, at: usize) -> bool {
        match self.keyword(at, &["Unset ", "unset "]) {
            Some(after) => self.try_flag(after, at, false),
            None => false,
        }
    }

    /// `FLAG`: `TARGETS END` after `Set`/`Unset` (at `after`).
    fn try_flag(&mut self, after: usize, at: usize, value: bool) -> bool {
        // A ` to ` later in the clause means a `Set … to` whose targets did
        // not parse ("Set the A and the B to v"), not a flag.
        let accept = |p: &Self, end: usize| {
            p.is_end(end)
                && p.find(end, " to ")
                    .is_none_or(|to| to >= p.value_end(end, false))
        };
        let Some((targets, end)) = self.targets(after, false, accept) else {
            return false;
        };
        if targets[0].path.hops.is_empty() || targets[0].path.subscript.is_some() {
            return false;
        }
        let kind = StatementKind::Set {
            targets: targets.iter().map(|t| t.path.clone()).collect(),
            value: Expr::Literal(Literal::Bool(value)),
            form: SetForm::Flag,
        };
        let id = self.push(at, end, "set", kind);
        self.target_roles(&targets, &id);
        self.covered_until = end;
        true
    }

    fn try_incdec(&mut self, at: usize) -> bool {
        let (op, after) = if let Some(after) = self.keyword(at, &["Increment ", "increment "]) {
            (MutationOp::Increment, after)
        } else if let Some(after) = self.keyword(at, &["Decrement ", "decrement "]) {
            (MutationOp::Decrement, after)
        } else {
            return false;
        };
        let Some(target) = self.path(after, false) else {
            return false;
        };
        let (operand, end) = if self.lit(target.end, " by ") {
            let value_start = target.end + " by ".len();
            let value_end = self.value_end(value_start, false);
            (Some((value_start, value_end)), value_end)
        } else if self.is_end(target.end) {
            (None, target.end)
        } else {
            return false;
        };
        let kind = StatementKind::Mutate {
            op,
            target: target.path.clone(),
            operand: operand.map(|(start, end)| self.expr(start, end)),
            basis: OpBasis::Verb,
        };
        let id = self.push(at, end, "mutate", kind);
        self.target_roles(std::slice::from_ref(&target), &id);
        if let Some((start, end)) = operand {
            self.read_roles(start, end, &id);
        }
        self.covered_until = end;
        true
    }

    fn try_let(&mut self, at: usize) -> bool {
        let Some(after) = self.keyword(at, &["Let ", "let "]) else {
            return false;
        };
        let Some((Placeholder::Var(first), mut pos)) = self.enc.placeholder(after) else {
            return false;
        };
        let mut vars = vec![first];
        if self.lit(pos, " and ") {
            if let Some((Placeholder::Var(second), end)) = self.enc.placeholder(pos + 5) {
                vars.push(second);
                pos = end;
            }
        }
        let Some(value_start) = self.keyword(pos, &[" be "]) else {
            return false;
        };
        let value_end = self.value_end(value_start, false);
        let value = self.expr(value_start, value_end);
        let mut first_id = None;
        for var in vars {
            let name = self.enc.vars[var].clone();
            let kind_name = format!("let:{name}");
            let kind = StatementKind::Let {
                var: name,
                value: value.clone(),
            };
            let id = self.push(at, value_end, &kind_name, kind);
            first_id.get_or_insert(id);
        }
        if let Some(id) = first_id {
            self.read_roles(value_start, value_end, &id);
        }
        self.covered_until = value_end;
        true
    }

    fn try_passive(&mut self, at: usize) -> bool {
        let Some(target) = self.path(at, false) else {
            return false;
        };
        let Some(value_start) = self.keyword(target.end, &[" must be set to "]) else {
            return false;
        };
        let value_end = self.value_end(value_start, false);
        let kind = StatementKind::Set {
            targets: vec![target.path.clone()],
            value: self.expr(value_start, value_end),
            form: SetForm::Passive,
        };
        let id = self.push(at, value_end, "set", kind);
        self.target_roles(std::slice::from_ref(&target), &id);
        self.read_roles(value_start, value_end, &id);
        self.covered_until = value_end;
        true
    }

    /// An `Opaque` statement for the clause at `at`. `target_text` is the
    /// text after the verb up to ` to ` or the value end, links elided.
    fn push_opaque(&mut self, at: usize, reason: OpaqueReason, verb: Option<String>) -> String {
        let after = match &verb {
            Some(verb) => {
                let end = at + verb.len();
                end + usize::from(self.lit(end, " "))
            }
            None => at,
        };
        let end = self.value_end(after, false);
        let target_end = self
            .find(after, " to ")
            .filter(|&to| to < end)
            .unwrap_or(end);
        let target_text = verb.is_some().then(|| self.render(after, target_end));
        let kind = StatementKind::Opaque {
            reason,
            verb,
            target_text,
        };
        self.opaque_clauses.push(at);
        self.push(at, end, "opaque", kind)
    }

    fn push(&mut self, start: usize, end: usize, kind_name: &str, kind: StatementKind) -> String {
        let span = self.enc.span(start, end);
        let id = statement_id(&self.source.id, kind_name, span);
        self.out.statements.push(Statement {
            id: id.clone(),
            source_id: self.source.id.clone(),
            span,
            kind,
        });
        id
    }

    fn role(&mut self, link: usize, class: OccurrenceClass, id: &str) {
        self.out
            .link_roles
            .entry(link)
            .or_insert_with(|| (class, id.to_string()));
    }

    /// Last hop of each target → `Write`; the root link and other hops →
    /// `ReadPath`; `ROOT'` phrases and subscripts → `Read`.
    fn target_roles(&mut self, targets: &[PathParse], id: &str) {
        for target in targets {
            if let Some((last, prefix)) = target.hop_links.split_last() {
                for link in prefix.iter().flatten() {
                    self.role(*link, OccurrenceClass::ReadPath, id);
                }
                if let Some(link) = last {
                    self.role(*link, OccurrenceClass::Write, id);
                }
            }
            if let Some(link) = target.root_link {
                self.role(link, OccurrenceClass::ReadPath, id);
            }
            for &(start, end) in &target.read_ranges {
                self.read_roles(start, end, id);
            }
        }
    }

    fn read_roles(&mut self, start: usize, end: usize, id: &str) {
        let links: Vec<usize> = self.enc.links_in(start, end).collect();
        for link in links {
            self.role(link, OccurrenceClass::Read, id);
        }
    }
}

/// A chained continuation target (`… to x and B to y`) starting at `start`.
struct Chained {
    start: usize,
    path: PathParse,
}

/// Grammar helpers over the encoded text.
impl Parser<'_> {
    fn lit(&self, pos: usize, s: &str) -> bool {
        self.enc.text[pos..].starts_with(s)
    }

    /// The position after the first of `words` that starts at `pos`.
    fn keyword(&self, pos: usize, words: &[&str]) -> Option<usize> {
        words
            .iter()
            .find(|word| self.lit(pos, word))
            .map(|word| pos + word.len())
    }

    fn find(&self, pos: usize, s: &str) -> Option<usize> {
        self.enc.text[pos..].find(s).map(|found| pos + found)
    }

    /// Canonical source text of an encoded range.
    fn src_text(&self, start: usize, end: usize) -> String {
        let span = self.enc.span(start, end);
        self.source.text[span.start..span.end].to_string()
    }

    /// Encoded range rendered with every link replaced by `_`.
    fn render(&self, start: usize, end: usize) -> String {
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
    fn is_end(&self, pos: usize) -> bool {
        pos == self.enc.text.len()
            || [".", ",", ";", " and ", " if ", " otherwise"]
                .iter()
                .any(|end| self.lit(pos, end))
    }

    fn is_this_link(&self, link: usize) -> bool {
        self.source.links[link]
            .target
            .as_ref()
            .is_some_and(|target| {
                target.spec.eq_ignore_ascii_case("WEBIDL") && target.anchor == "this"
            })
    }

    fn field_hop(&self, link: usize) -> Hop {
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
    fn targets(
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
                let Some((hop, Some(link), after)) = self.hop(next) else {
                    break;
                };
                end = self.trailer(after);
                let mut path = first.path.clone();
                path.hops.pop();
                path.hops.push(hop);
                let mut hop_links = first.hop_links.clone();
                hop_links.pop();
                hop_links.push(Some(link));
                more.push(PathParse {
                    path,
                    end,
                    hop_links,
                    root_link: first.root_link,
                    read_ranges: Vec::new(),
                });
            }
        }
        if !accept(self, end) {
            return None;
        }
        let mut targets = vec![first];
        targets.append(&mut more);
        Some((targets, end))
    }

    /// `PATH`. With `to_terminated`, also `the ⟦L⟧ of ROOT'` where `ROOT'`
    /// runs up to ` to `.
    fn path(&self, pos: usize, to_terminated: bool) -> Option<PathParse> {
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
            if let Some(after) = self.keyword(at, &["'s ", "’s "]) {
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
        if let RootKind::Link(link) = kind {
            if hops.is_empty() {
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
            subscript = Some(Box::new(self.expr(at + 1, close)));
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

    fn of_path(&self, pos: usize) -> Option<PathParse> {
        let the = self.keyword(pos, &["the "]).unwrap_or(pos);
        let (Placeholder::Link(link), after) = self.enc.placeholder(the)? else {
            return None;
        };
        let root_start = self.keyword(after, &[" of "])?;
        let root_end = self.find(root_start, " to ")?;
        let root = match self.root(root_start) {
            Some((root, RootKind::Var | RootKind::This, end)) if end == root_end => root,
            Some((root @ Root::Link { .. }, RootKind::Link(_), end)) if end == root_end => root,
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

    /// `ROOT`: `⟦V⟧`, `this`, `the`? `⟦L⟧`, a pronoun, or a 1–6 word phrase
    /// followed by `POSS HOP`.
    fn root(&self, pos: usize) -> Option<(Root, RootKind, usize)> {
        if let Some(end) = self.keyword(pos, &["its ", "their "]) {
            let text = self.enc.text[pos..end - 1].to_string();
            return Some((Root::Opaque { text }, RootKind::Pronoun, end));
        }
        let direct = self.direct_root(pos);
        if let Some((_, _, end)) = direct {
            if self.keyword(end, &["'s ", "’s ", ".[["]).is_some() {
                return direct;
            }
        }
        // "this element's F", "the ⟦`img`⟧ element's F"
        self.phrase_root(pos).or(direct)
    }

    /// `⟦V⟧`, `this`, or `the`? `⟦L⟧` / `the ⟦V⟧`.
    fn direct_root(&self, pos: usize) -> Option<(Root, RootKind, usize)> {
        let at = self.keyword(pos, &["the "]).unwrap_or(pos);
        match self.enc.placeholder(at) {
            Some((Placeholder::Var(var), end)) => {
                Some((Root::Var(self.enc.vars[var].clone()), RootKind::Var, end))
            }
            Some((Placeholder::Link(link), end)) => Some(self.link_root(link, end)),
            None if at == pos && self.lit(pos, "this") && !self.word_continues(pos + 4) => {
                Some((Root::This, RootKind::This, pos + 4))
            }
            None => None,
        }
    }

    fn link_root(&self, link: usize, end: usize) -> (Root, RootKind, usize) {
        if self.is_this_link(link) {
            return (Root::This, RootKind::This, end);
        }
        let root = Root::Link {
            link_id: self.source.links[link].id.clone(),
            target: self.source.links[link].target.clone(),
        };
        (root, RootKind::Link(link), end)
    }

    fn phrase_root(&self, pos: usize) -> Option<(Root, RootKind, usize)> {
        let mut at = self.keyword(pos, &["the "]).unwrap_or(pos);
        // "set the element's F" is a clause, not a phrase "set the element".
        if at == pos && self.enc.verb_at(pos).is_some() {
            return None;
        }
        for _ in 0..6 {
            let word_end = self.word_end(at)?;
            if let Some(after) = self.keyword(word_end, &["'s ", "’s "]) {
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
    fn word_end(&self, pos: usize) -> Option<usize> {
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

    fn word_continues(&self, pos: usize) -> bool {
        self.enc
            .text
            .as_bytes()
            .get(pos)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    }

    /// `HOP`: `⟦L⟧`, or `` `name` attribute`` with an optional ` value`. A
    /// linked code member (`` ⟦`name`⟧ attribute``) is a field hop.
    fn hop(&self, pos: usize) -> Option<(Hop, Option<usize>, usize)> {
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
    fn attribute_suffix(&self, pos: usize) -> Option<usize> {
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
    fn code(&self, pos: usize) -> Option<(String, usize)> {
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
    fn slot(&self, pos: usize) -> Option<(String, usize)> {
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
    fn trailer(&self, pos: usize) -> usize {
        [" flags", " flag", " state", " boolean"]
            .iter()
            .find(|word| self.lit(pos, word) && !self.word_continues(pos + word.len()))
            .map_or(pos, |word| pos + word.len())
    }

    fn matching_bracket(&self, open: usize) -> Option<usize> {
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

    /// End of a `VALUE` starting at `start`. With `chain`, a chained `Set`
    /// continuation also ends it.
    fn value_end(&self, start: usize, chain: bool) -> usize {
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
            if chain && self.chained_continuation(i).is_some() {
                return i;
            }
        }
        text.len()
    }

    fn verb_follows(&self, pos: usize) -> bool {
        self.enc
            .verb_at(pos)
            .is_some_and(|verb| self.lit(pos + verb.len(), " "))
    }

    /// `PATH " to "` at `pos`: the target of a chained continuation.
    fn chained_path(&self, pos: usize) -> Option<PathParse> {
        self.path(pos, true)
            .filter(|path| self.lit(path.end, " to "))
    }

    /// `(", and " | ", " | " and ") PATH " to "` at `end`.
    fn chained_continuation(&self, end: usize) -> Option<Chained> {
        let start = self.keyword(end, &[", and ", ", ", " and "])?;
        let path = self.chained_path(start)?;
        Some(Chained { start, path })
    }

    /// A value that starts with `be ` and a link, or with a link whose text
    /// starts with `be `, is an invocation.
    fn is_invocation(&self, pos: usize) -> bool {
        if let Some(after) = self.keyword(pos, &["be "]) {
            if matches!(self.enc.placeholder(after), Some((Placeholder::Link(_), _))) {
                return true;
            }
        }
        matches!(self.enc.placeholder(pos), Some((Placeholder::Link(link), _))
            if self.source.links[link].visible_text.starts_with("be "))
    }

    /// `VALUE` → `Expr`.
    fn expr(&self, start: usize, end: usize) -> Expr {
        let text = &self.enc.text[start..end];
        let start = start + (text.len() - text.trim_start().len());
        let end = end - (text.len() - text.trim_end().len());
        let text = &self.enc.text[start.min(end)..end];
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
        if let Some(literal) = literal(text) {
            return Expr::Literal(literal);
        }
        if let Some((inner, after)) = self.code(start) {
            if after == end {
                if let Some(literal) = literal(&inner) {
                    return Expr::Literal(literal);
                }
            }
        }
        if let Some(after) = self.keyword(start, &["a new ", "an new "]) {
            return Expr::New {
                ty: self.new_type(after),
                init: None,
            };
        }
        if let Some(path) = self.path(start, false).filter(|path| path.end == end) {
            return Expr::Path(path.path);
        }
        Expr::Opaque {
            text: self.src_text(start, end),
        }
    }

    /// `NEWTYPE`: a link, or a code token naming an IDL interface.
    fn new_type(&self, pos: usize) -> Option<TypeRef> {
        if let Some((Placeholder::Link(link), _)) = self.enc.placeholder(pos) {
            return self.source.links[link]
                .target
                .clone()
                .map(TypeRef::Unresolved);
        }
        self.code(pos)
            .map(|(name, _)| TypeRef::Known(TypeKey::Idl(name)))
    }
}

/// `true`/`false`/`null`/`undefined`, a number, or a quoted string.
fn literal(text: &str) -> Option<Literal> {
    match text {
        "true" => return Some(Literal::Bool(true)),
        "false" => return Some(Literal::Bool(false)),
        "null" | "undefined" => return Some(Literal::Null),
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
    for (open, close) in [('"', '"'), ('“', '”')] {
        if let Some(inner) = text.strip_prefix(open).and_then(|t| t.strip_suffix(close)) {
            return Some(Literal::String(inner.to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::steps::extract_step_structure;

    /// Wrap step bodies in an algorithm, extract structure, return the sources of all segments.
    fn sources(steps: &[&str]) -> Vec<StatementSource> {
        let lis: String = steps
            .iter()
            .map(|s| format!("<li><p>{s}</p></li>"))
            .collect();
        let html = format!(
            r#"<div class="algorithm"><p>To <dfn id="algo">algo</dfn>:</p><ol>{lis}</ol></div>"#
        );
        let structure =
            extract_step_structure(&html, "HTML", "https://html.spec.whatwg.org/", "hash:t");
        crate::state::extract::algorithm_sources(&structure)
    }

    fn one(step: &str) -> (StatementSource, ParsedSource) {
        let source = sources(&[step]).remove(0);
        let parsed = parse_source(&source);
        (source, parsed)
    }

    fn role_of(
        source: &StatementSource,
        parsed: &ParsedSource,
        visible: &str,
    ) -> Option<OccurrenceClass> {
        let index = source
            .links
            .iter()
            .position(|l| l.visible_text == visible)?;
        parsed.link_roles.get(&index).map(|(class, _)| *class)
    }

    fn nth_role(
        source: &StatementSource,
        parsed: &ParsedSource,
        visible: &str,
        n: usize,
    ) -> Option<OccurrenceClass> {
        let index = source
            .links
            .iter()
            .enumerate()
            .filter(|(_, l)| l.visible_text == visible)
            .nth(n)?
            .0;
        parsed.link_roles.get(&index).map(|(class, _)| *class)
    }

    #[test]
    fn local_set_reads_the_field_in_value_position() {
        let (s, p) = one(
            r##"Set <var>x</var> to <var>nav</var>'s <a href="#nav-document">active document</a>."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if targets[0].root == Root::Var("x".into()) && targets[0].hops.is_empty())
        );
        assert_eq!(
            role_of(&s, &p, "active document"),
            Some(OccurrenceClass::Read)
        );
    }

    #[test]
    fn field_write_with_literal_value() {
        let (s, p) = one(
            r##"Set <var>document</var>'s <a href="#is-initial-about:blank">is initial <code>about:blank</code></a> to false."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Set {
                value: Expr::Literal(Literal::Bool(false)),
                form: SetForm::To,
                ..
            }
        ));
        assert_eq!(
            role_of(&s, &p, "is initial `about:blank`"),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn receiver_less_set_is_implicit() {
        let (s, p) =
            one(r##"Set the <a href="#insertion-point">insertion point</a> to undefined."##);
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if targets[0].root == Root::Implicit)
        );
        assert_eq!(
            role_of(&s, &p, "insertion point"),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn set_the_field_of_a_quantified_phrase() {
        let (s, p) = one(
            r##"Set the <a href="#concept-node-document">node document</a> of each attribute in <var>x</var>’s <a href="#concept-element-attribute">attribute list</a> to <var>d</var>."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if matches!(targets[0].root, Root::Opaque { .. }))
        );
        assert_eq!(
            role_of(&s, &p, "node document"),
            Some(OccurrenceClass::Write)
        );
        assert_eq!(
            role_of(&s, &p, "attribute list"),
            Some(OccurrenceClass::Read)
        );
    }

    #[test]
    fn flag_forms_and_target_lists() {
        let (s, p) = one(
            r##"Set <var>e</var>’s <a href="#stop-propagation-flag">stop propagation flag</a>."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Set {
                form: SetForm::Flag,
                value: Expr::Literal(Literal::Bool(true)),
                ..
            }
        ));
        assert_eq!(
            role_of(&s, &p, "stop propagation flag"),
            Some(OccurrenceClass::Write)
        );
        let (s, p) = one(
            r##"Unset <var>e</var>’s <a href="#dispatch-flag">dispatch flag</a>, <a href="#stop-propagation-flag">stop propagation flag</a>, and <a href="#sipf">stop immediate propagation flag</a>."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, value: Expr::Literal(Literal::Bool(false)), .. } if targets.len() == 3)
        );
        assert_eq!(
            role_of(&s, &p, "stop immediate propagation flag"),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn decrement_with_phrase_root() {
        let (s, p) = one(
            r##"Decrement the parser's <a href="#script-nesting-level">script nesting level</a> by one."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Mutate {
                op: MutationOp::Decrement,
                basis: OpBasis::Verb,
                ..
            }
        ));
        assert_eq!(
            role_of(&s, &p, "script nesting level"),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn value_starting_with_be_link_is_an_invocation() {
        let (s, p) = one(
            r##"Set <var>subject</var>'s <a href="#concept-node-document">node document</a> to <a href="#blocked-by-a-modal-dialog">be blocked by the modal dialog</a> <var>subject</var>."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Opaque {
                reason: OpaqueReason::ValueIsInvocation,
                ..
            }
        ));
        assert_eq!(
            role_of(&s, &p, "node document"),
            Some(OccurrenceClass::ReadPath)
        );
    }

    #[test]
    fn condition_read_then_trailer_write() {
        let (s, p) = one(
            r##"If <var>d</var>'s <a href="#concept-document-salvageable">salvageable</a> is false, then set <var>e</var>'s <a href="#concept-document-salvageable">salvageable</a> state to false."##,
        );
        assert_eq!(
            nth_role(&s, &p, "salvageable", 0),
            None,
            "condition links get no statement role; classify makes them read"
        );
        assert_eq!(
            nth_role(&s, &p, "salvageable", 1),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn chained_second_target_in_one_sentence() {
        let (s, p) = one(
            r##"Set <a href="https://webidl.spec.whatwg.org/#this">this</a>’s <a href="#concept-cd-data">data</a> to <var>data</var> and <a href="https://webidl.spec.whatwg.org/#this">this</a>’s <a href="#concept-node-document">node document</a> to <a href="#current-global-object">current global object</a>’s <a href="#concept-document-window">associated <code>Document</code></a>."##,
        );
        assert_eq!(p.statements.len(), 2);
        assert!(matches!(
            &p.statements[1].kind,
            StatementKind::Set {
                form: SetForm::Chained,
                ..
            }
        ));
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if targets[0].root == Root::This)
        );
        assert_eq!(
            role_of(&s, &p, "node document"),
            Some(OccurrenceClass::Write)
        );
        assert_eq!(role_of(&s, &p, "data"), Some(OccurrenceClass::Write));
    }

    #[test]
    fn pronoun_root_with_linked_hop_is_a_write() {
        let (s, p) = one(
            r##"Set its <a href="#concept-range-start-node">start node</a> to <var>node</var>."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if matches!(&targets[0].root, Root::Opaque { text } if text == "its"))
        );
        assert_eq!(role_of(&s, &p, "start node"), Some(OccurrenceClass::Write));
    }

    #[test]
    fn spans_are_utf8_boundaries_of_the_canonical_text() {
        let (s, p) = one(
            r##"Unset <var>e</var>’s <a href="#dispatch-flag">dispatch flag</a> and set <var>e</var>’s <a href="#x">x</a> to «&nbsp;»."##,
        );
        assert_eq!(p.statements.len(), 2);
        for statement in &p.statements {
            assert!(
                s.text.is_char_boundary(statement.span.start)
                    && s.text.is_char_boundary(statement.span.end)
            );
            let _ = &s.text[statement.span.start..statement.span.end];
        }
    }

    #[test]
    fn phrase_roots_with_this_links_and_code() {
        let (s, p) = one(r##"Set this element's <a href="#dirty">dirty value flag</a> to true."##);
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if matches!(&targets[0].root, Root::Opaque { text } if text == "this element"))
        );
        assert_eq!(
            role_of(&s, &p, "dirty value flag"),
            Some(OccurrenceClass::Write)
        );
        let (s, p) = one(
            r##"Set the <code><a href="#the-img-element">img</a></code> element's <a href="#current-request">current request</a> to null."##,
        );
        assert_eq!(
            role_of(&s, &p, "current request"),
            Some(OccurrenceClass::Write)
        );
        assert_eq!(role_of(&s, &p, "`img`"), Some(OccurrenceClass::Read));
        let (s, p) =
            one(r##"Set the <var>image request</var>'s <a href="#img-req-state">state</a> to 1."##);
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if targets[0].root == Root::Var("image request".into()))
        );
        assert_eq!(role_of(&s, &p, "state"), Some(OccurrenceClass::Write));
    }

    #[test]
    fn receiver_less_forms() {
        let (s, p) =
            one(r##"Set <a href="#is-modal">is modal</a> of <var>subject</var> to true."##);
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if targets[0].root == Root::Var("subject".into()))
        );
        assert_eq!(role_of(&s, &p, "is modal"), Some(OccurrenceClass::Write));
        let (s, p) = one(
            r##"Set the <code><a href="#dom-media-paused">paused</a></code> attribute to true."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if targets[0].root == Root::Implicit)
        );
        assert_eq!(role_of(&s, &p, "`paused`"), Some(OccurrenceClass::Write));
        let (s, p) = one(
            r##"Set the <a href="#insertion-point">insertion point</a> to <var>nav</var>'s <a href="#f">f</a>."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Set {
                form: SetForm::To,
                ..
            }
        ));
        assert_eq!(
            role_of(&s, &p, "insertion point"),
            Some(OccurrenceClass::Write)
        );
        assert_eq!(role_of(&s, &p, "f"), Some(OccurrenceClass::Read));
    }

    #[test]
    fn comma_chains_and_receiver_less_target_lists() {
        let (s, p) = one(
            r##"Set its <a href="#k">kind</a> to <var>kind</var>, its <a href="#l">label</a> to <var>label</var>, and <var>e</var>[<var>n</var>] to null."##,
        );
        assert_eq!(p.statements.len(), 3);
        assert_eq!(role_of(&s, &p, "label"), Some(OccurrenceClass::Write));
        assert!(matches!(
            &p.statements[2].kind,
            StatementKind::Mutate {
                op: MutationOp::MapSet,
                ..
            }
        ));
        let (s, p) = one(
            r##"Set the <a href="#cpp">current playback position</a> and the <a href="#opp">official playback position</a> to <var>t</var>."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, form: SetForm::To, .. } if targets.len() == 2)
        );
        assert_eq!(
            role_of(&s, &p, "official playback position"),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn comma_verb_starts_a_new_statement() {
        let (s, p) = one(
            r##"Set <var>e</var>'s <a href="#ns">networkState</a> to 0, set the element's <a href="#sp">show poster flag</a> to true, and fire an event."##,
        );
        assert_eq!(p.statements.len(), 2);
        assert!(
            matches!(&p.statements[1].kind, StatementKind::Set { targets, form: SetForm::To, .. } if matches!(&targets[0].root, Root::Opaque { text } if text == "the element"))
        );
        assert_eq!(
            role_of(&s, &p, "show poster flag"),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn target_lists_parse_whole_or_not_at_all() {
        let (s, p) = one(
            r##"Set the new text track's <a href="#k">kind</a>, <a href="#l">label</a>, and <a href="#i">identifier</a> based on the data."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Opaque {
                reason: OpaqueReason::UnparsedTarget,
                ..
            }
        ));
        assert_eq!(role_of(&s, &p, "kind"), None);
        let (s, p) = one(
            r##"Set the <code><a href="#w">videoWidth</a></code> and <code><a href="#h">videoHeight</a></code> attributes, and return."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, form: SetForm::Flag, .. } if targets.len() == 2)
        );
        assert_eq!(
            role_of(&s, &p, "`videoHeight`"),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn flag_form_needs_no_later_to_in_the_clause() {
        let (_, p) = one(
            r##"Set the <a href="#cpp">current playback position</a> and more things to <var>t</var>."##,
        );
        assert!(!p.statements.iter().any(|st| matches!(
            st.kind,
            StatementKind::Set {
                form: SetForm::Flag,
                ..
            }
        )));
        let (s, p) = one(
            r##"Set <var>e</var>'s <a href="#canceled">canceled flag</a>, and set <var>x</var> to 1."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Set {
                form: SetForm::Flag,
                ..
            }
        ));
        assert_eq!(
            role_of(&s, &p, "canceled flag"),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn let_binds_each_variable_and_reads_its_value() {
        let (s, p) = one(
            r##"Let <var>a</var> and <var>b</var> be <var>doc</var>'s <a href="#concept-document-origin">origin</a>."##,
        );
        assert_eq!(p.statements.len(), 2);
        assert!(
            matches!(&p.statements[1].kind, StatementKind::Let { var, value: Expr::Path(_) } if var == "b")
        );
        assert_ne!(p.statements[0].id, p.statements[1].id);
        assert_eq!(role_of(&s, &p, "origin"), Some(OccurrenceClass::Read));
        assert!(p.clauses[0].consumed);
    }

    #[test]
    fn passive_set_and_map_set() {
        let (s, p) =
            one(r##"<var>x</var>'s <a href="#f">f</a> must be set to <code>null</code>."##);
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Set {
                form: SetForm::Passive,
                value: Expr::Literal(Literal::Null),
                ..
            }
        ));
        assert_eq!(role_of(&s, &p, "f"), Some(OccurrenceClass::Write));
        let (s, p) = one(
            r##"Set <var>x</var>'s <a href="#m">map</a>[<var>k</var>'s <a href="#n">name</a>] to 1."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Mutate { op: MutationOp::MapSet, operand: Some(Expr::Literal(Literal::Number(n))), target, .. } if n == "1" && target.subscript.is_some())
        );
        assert_eq!(role_of(&s, &p, "map"), Some(OccurrenceClass::Write));
        assert_eq!(role_of(&s, &p, "name"), Some(OccurrenceClass::Read));
    }

    #[test]
    fn increment_code_member_and_slot_hops() {
        let (s, p) =
            one(r##"Increment <var>e</var>'s <a href="#a">a</a>'s <code>count</code> attribute."##);
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Mutate { op: MutationOp::Increment, target, operand: None, .. } if target.hops.last() == Some(&Hop::CodeMember { name: "count".into() }))
        );
        assert_eq!(role_of(&s, &p, "a"), Some(OccurrenceClass::ReadPath));
        let (s, p) = one(
            r##"Set <var>event</var>’s <code><a href="#dom-event-istrusted">isTrusted</a></code> attribute to false."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, .. } if matches!(targets[0].hops[..], [Hop::Field { .. }]))
        );
        assert_eq!(role_of(&s, &p, "`isTrusted`"), Some(OccurrenceClass::Write));
        let (_, p) = one(r##"Set <var>o</var>.[[Realm]] to <var>r</var>."##);
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, value: Expr::Var(v), .. } if v == "r" && targets[0].hops == vec![Hop::Slot { name: "Realm".into() }])
        );
        let (_, p) =
            one(r##"Set <var>o</var>.<var class="field">[[Done]]</var> to <code>true</code>."##);
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, value: Expr::Literal(Literal::Bool(true)), .. } if targets[0].hops == vec![Hop::Slot { name: "Done".into() }])
        );
    }

    #[test]
    fn unparsed_and_unsupported_clauses_are_opaque() {
        let (s, p) = one(
            r##"Set the thing described by <a href="#y">y</a> to 1, then append <var>x</var> to <var>s</var>'s <a href="#l">list</a>."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Opaque { reason: OpaqueReason::UnparsedTarget, verb: Some(v), target_text: Some(t) } if v == "set" && t == "the thing described by _"
        ));
        assert!(matches!(
            &p.statements[1].kind,
            StatementKind::Opaque { reason: OpaqueReason::UnsupportedForm, verb: Some(v), target_text: Some(t) } if v == "append" && t == "*x*"
        ));
        assert_eq!(p.statements.len(), 2);
        assert_eq!(role_of(&s, &p, "y"), None);
        let verbs: Vec<_> = p.clauses.iter().filter_map(|c| c.verb.as_deref()).collect();
        assert_eq!(verbs, ["set", "append"]);
        assert!(p.clauses.iter().all(|c| !c.consumed));
    }

    #[test]
    fn value_ends_before_a_following_verb_clause() {
        let (s, p) = one(
            r##"Set <var>x</var> to <var>y</var>'s <a href="#a">a</a>, and set <var>z</var>'s <a href="#b">b</a> to true."##,
        );
        assert_eq!(p.statements.len(), 2);
        assert_eq!(role_of(&s, &p, "a"), Some(OccurrenceClass::Read));
        assert_eq!(role_of(&s, &p, "b"), Some(OccurrenceClass::Write));
        let clause = p
            .clauses
            .iter()
            .find(|c| c.start == p.statements[1].span.start);
        assert!(clause.is_some_and(|c| c.consumed));
    }
}
