//! Minimal statement IR (§7.1 sources, §7.2 statements and expressions) and
//! its grammar (§7.3).
use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::parse::steps::{AnchorTarget, InlineToken, LinkSpan, TextSpan};
use crate::state::grammar::{
    Callability, Encoded, Env, InfraTarget, Parser, PathParse, Placeholder,
};
use crate::state::model::{Literal, OccurrenceClass, TypeRef};

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
    /// A `<dt>` entry of a `<dl>` initializer. `segment_id` is the segment
    /// introducing the initializer, `value_segment_id` the branch's first
    /// segment, which holds the entry value.
    BranchLabel {
        branch_id: String,
        step_id: String,
        step_path: String,
        segment_id: String,
        #[serde(default)]
        value_segment_id: Option<String>,
    },
    Prose {
        node_id: String,
        step_path: Option<String>,
        role: ProseRole,
    },
    /// The algorithm introduction paragraph (§8 intro source).
    Intro {
        algorithm: AnchorTarget,
        node_id: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<StatementParent>,
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
    /// A call statement; `call` is the `Call.id`.
    Call {
        call: String,
    },
    If {
        condition: Predicate,
        then: BodyLoc,
    },
    Otherwise {
        of: Option<String>,
        condition: Option<Predicate>,
        body: BodyLoc,
    },
    ForEach {
        vars: Vec<String>,
        collection: Expr,
        filter: Option<Predicate>,
        order: Option<String>,
        body: BodyLoc,
    },
    While {
        condition: Option<Predicate>,
        form: LoopForm,
        body: BodyLoc,
    },
    Return {
        value: Option<Expr>,
    },
    Throw {
        exception: ExceptionRef,
    },
    Abort {
        text: String,
    },
    Continue,
    Break,
    ContinueRemaining {
        continuation_site: Option<String>,
    },
    Wait {
        condition: Option<Predicate>,
        text: String,
    },
    InParallel {
        body: BodyLoc,
    },
    RunSteps {
        body: BodyLoc,
    },
    Assert {
        predicate: Predicate,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// A call result; value is the `Call.id`.
    Call(String),
    /// An algorithm named as a value (not a call).
    AlgorithmRef {
        link_id: String,
        target: Option<AnchorTarget>,
    },
    /// A list literal `« e1, e2, … »`.
    List(Vec<Expr>),
    /// An enum string value, e.g. `"auto"`.
    EnumValue {
        text: String,
        target: Option<AnchorTarget>,
    },
    Conditional {
        condition: Box<Predicate>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
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
// New statement-level types (§F2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementParent {
    pub statement_id: String,
    pub role: BlockRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockRole {
    Then,
    Otherwise,
    Body,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyLoc {
    InlineRest,
    ChildSteps { step_id: String },
    Body { body_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopForm {
    While,
    Repeat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionRef {
    pub name: Option<String>,
    pub link: Option<AnchorTarget>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    Is {
        operand: Expr,
        test: Test,
        negated: bool,
    },
    Compare {
        lhs: Expr,
        op: CmpOp,
        rhs: Expr,
        negated: bool,
    },
    OneOf {
        operand: Expr,
        values: Vec<Expr>,
        negated: bool,
    },
    Contains {
        container: Expr,
        item: Expr,
        negated: bool,
    },
    Exists {
        operand: Expr,
        negated: bool,
    },
    HasAttribute {
        element: Expr,
        name: String,
        negated: bool,
    },
    Holds {
        subjects: Vec<Expr>,
        link_id: String,
        target: Option<AnchorTarget>,
        call: Option<String>,
        negated: bool,
    },
    RunningOn {
        context: RunContext,
    },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Implies(Box<Predicate>, Box<Predicate>),
    Opaque {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Test {
    Null,
    True,
    False,
    Empty,
    Set,
    Type(crate::state::model::TypeExpr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmpOp {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunContext {
    InParallel,
    Queue(Expr),
    EventLoopTask(Expr),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub id: String,
    pub source_id: String,
    pub statement_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_call: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nested: Vec<String>,
    pub callee: Callee,
    pub form: CallForm,
    pub span: TextSpan,
    pub region: TextSpan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<Expr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub named: Vec<NamedArg>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_args: Vec<String>,
    pub hint: ExecutionHint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Callee {
    pub link_id: String,
    pub target: Option<AnchorTarget>,
    pub visible_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallForm {
    Imperative,
    ResultOf,
    Gerund,
    Possessive,
    Predicate,
    SetToBe,
    Ecmarkup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedArg {
    pub name: ArgName,
    pub value: Expr,
    pub span: TextSpan,
    pub form: NamedForm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgName {
    ParamLink {
        link_id: String,
        target: Option<AnchorTarget>,
    },
    Var(String),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamedForm {
    SetTo,
    FlagSet,
    AttributeInit { member: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionHint {
    Inline,
    InParallel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLinkRoles {
    pub source_id: String,
    /// One role per link, in link order.
    pub roles: Vec<LinkRole>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkRole {
    Callee { call: String },
    AlgorithmValue,
    ParamName { call: String },
    Field,
    Type,
    Predicate { call: Option<String> },
    InfraOp { statement: String },
    Keyword,
    Value,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarOrigins {
    pub subject: AnchorTarget,
    pub vars: Vec<VarOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarOrigin {
    pub name: String,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Param { index: u32 },
    Let { statement_id: String },
    LoopVar { statement_id: String },
    BodyParam { body_id: String },
    Undeclared,
}

/// `call-` + sha256(source_id \0 link_id). A link is the callee of at most one call.
pub(crate) fn call_id(source_id: &str, link_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_id.as_bytes());
    hasher.update([0]);
    hasher.update(link_id.as_bytes());
    format!("call-{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// §7.3 Grammar
// ---------------------------------------------------------------------------

/// Clause-initial mutation verbs (§7.4). They decide between `read` and
/// `unclassified`; they never produce a write.
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

/// Infra-linked mutations (§7.3): `INFRA` anchor, operation, and the
/// prepositions that end the operand and start the target. Without
/// prepositions the target follows the link directly.
const INFRA_OPS: &[(&str, MutationOp, &[&str])] = &[
    ("list-append", MutationOp::Append, &[" to ", " into "]),
    ("set-append", MutationOp::Append, &[" to ", " into "]),
    ("list-prepend", MutationOp::Prepend, &[" to "]),
    ("set-prepend", MutationOp::Prepend, &[" to "]),
    ("list-extend", MutationOp::Extend, &[]),
    ("list-insert", MutationOp::Insert, &[" into "]),
    ("list-remove", MutationOp::Remove, &[" from "]),
    ("list-replace", MutationOp::Replace, &[" in "]),
    ("set-replace", MutationOp::Replace, &[" in "]),
    ("list-empty", MutationOp::Empty, &[]),
    ("map-set", MutationOp::MapSet, &[]),
    ("map-remove", MutationOp::MapRemove, &[]),
    ("map-clear", MutationOp::Clear, &[]),
    ("queue-enqueue", MutationOp::Enqueue, &[" to ", " on "]),
    ("queue-dequeue", MutationOp::Dequeue, &[" from "]),
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedSource {
    /// `Init` statements first, then the statements of each clause in order.
    pub statements: Vec<Statement>,
    /// link index (into `source.links`) → (class, statement id) for every
    /// link a statement positions.
    pub link_roles: BTreeMap<usize, (OccurrenceClass, String)>,
    pub clauses: Vec<Clause>,
    /// The calls of this source, with statements, parents and children.
    pub calls: Vec<Call>,
    /// link index → `LinkRole`; filled by grammar productions (Task F7+).
    pub roles: BTreeMap<usize, LinkRole>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Clause {
    /// Byte offset into the source text.
    pub start: usize,
    /// The clause-initial lexicon verb, or the text of a clause-initial
    /// Infra operation link, lowercase.
    pub verb: Option<String>,
    pub consumed: bool,
}

/// `stmt-` + sha256(source_id \0 kind \0 start \0 end).
pub(crate) fn statement_id(source_id: &str, kind: &str, span: TextSpan) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_id.as_bytes());
    for component in [kind, &span.start.to_string(), &span.end.to_string()] {
        hasher.update([0]);
        hasher.update(component.as_bytes());
    }
    format!("stmt-{:x}", hasher.finalize())
}

/// Parse one statement source (§7.3) with the given extraction environment.
/// Every clause gets a `Clause`; each clause that starts with a lexicon verb
/// and yields no structured statement gets an `Opaque` statement.
pub(crate) fn parse_source_with(source: &StatementSource, env: &Env) -> ParsedSource {
    let enc = Encoded::new(source);
    let mut p = Parser::new(&enc, source, env);
    p.inits = p.parse_initializers();
    let clause_starts = enc.clause_starts(is_prose(source));
    let mut sentences = p.sentence_starts().into_iter().peekable();
    for (at, verb) in clause_starts {
        while let Some(sentence) = sentences.next_if(|&sentence| sentence <= at) {
            p.sentence_boundary(sentence, sentence == at);
        }
        // An Infra-linked operation is spelled by its link text ("Append").
        let verb = verb.or_else(|| {
            p.infra_op(at)
                .map(|(link, ..)| source.links[link].visible_text.to_lowercase())
        });
        let consumed = at < p.covered_until
            || p.try_control(at)
            || p.try_call_statement(at)
            || p.try_infra_mutation(at)
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
    for sentence in sentences {
        p.sentence_boundary(sentence, false);
    }
    p.inherit_inline_blocks();
    p.scan_result_of_calls();
    p.finish_calls();
    p.out
}

/// Parse one statement source with the default (SP1) environment.
pub(crate) fn parse_source(source: &StatementSource) -> ParsedSource {
    parse_source_with(source, &Env::default())
}

fn is_prose(source: &StatementSource) -> bool {
    matches!(source.context, SourceContext::Prose { .. })
}

/// Whether some clause of `source` starts with a lexicon verb or an Infra
/// mutation link, or is a `PASSIVE` set (§7.5 statement source test).
pub(crate) fn has_mutation_clause(source: &StatementSource) -> bool {
    let enc = Encoded::new(source);
    let env = Env::default();
    let mut p = Parser::new(&enc, source, &env);
    enc.clause_starts(is_prose(source))
        .into_iter()
        .any(|(at, verb)| verb.is_some() || p.infra_op(at).is_some() || p.try_passive(at))
}

/// The constructed type of a segment that introduces a `<dl>` initializer
/// ("… a new ⟦Document⟧, with:"); `None` if the segment introduces none. The
/// inner `None` is a type link without a target.
pub(crate) fn initializer_intro(source: &StatementSource) -> Option<Option<TypeRef>> {
    static INTRO: OnceLock<Regex> = OnceLock::new();
    let intro = INTRO.get_or_init(|| {
        Regex::new(
            r"(?:a|an) new (?P<ty>⟦L\d+⟧|`[A-Za-z_]\w*`)(?: (?:node|object|element))?,? with:?$",
        )
        .expect("valid regex")
    });
    let enc = Encoded::new(source);
    let ty = intro.captures(&enc.text)?.name("ty")?.start();
    let env = Env::default();
    Some(Parser::new(&enc, source, &env).new_type(ty))
}

/// The whole source as one `VALUE`.
pub(crate) fn parse_value(source: &StatementSource) -> Expr {
    let enc = Encoded::new(source);
    let env = Env::default();
    let mut p = Parser::new(&enc, source, &env);
    let end = p.value_end(0, None);
    p.expr_at(0, end)
}

/// The links a parsed `PATH` positions, as indices into `source.links`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PathRoles {
    /// The last hop's link.
    pub write: Option<usize>,
    /// The root link and the other hop links.
    pub read_path: Vec<usize>,
    /// Links of `ROOT'` phrases and subscripts.
    pub read: Vec<usize>,
}

/// `span` of the source text as one complete `PATH` (§7.3), for rule
/// captures (§8.3); `None` if the path grammar does not cover the whole span.
pub(crate) fn parse_path_span(
    source: &StatementSource,
    span: TextSpan,
) -> Option<(Path, PathRoles)> {
    let enc = Encoded::new(source);
    let env = Env::default();
    let p = Parser::new(&enc, source, &env);
    let text = source.text.get(span.start..span.end)?;
    let start = enc.to_enc(span.start + (text.len() - text.trim_start().len()));
    let end = enc.to_enc(span.end - (text.len() - text.trim_end().len()));
    let parsed = p.path(start, false).filter(|path| path.end == end)?;
    let mut roles = PathRoles::default();
    if let Some((last, prefix)) = parsed.hop_links.split_last() {
        roles.write = *last;
        roles.read_path.extend(prefix.iter().flatten());
    }
    roles.read_path.extend(parsed.root_link);
    for &(start, end) in &parsed.read_ranges {
        roles.read.extend(enc.links_in(start, end));
    }
    Some((parsed.path, roles))
}

/// `span` of the source text as one `VALUE`.
pub(crate) fn parse_expr_span(source: &StatementSource, span: TextSpan) -> Expr {
    let enc = Encoded::new(source);
    let env = Env::default();
    let start = enc.to_enc(span.start);
    let end = enc.to_enc(span.end);
    Parser::new(&enc, source, &env).expr_at(start, end)
}

/// A `<dl>` initializer entry (§7.3): the label's first link is the field,
/// `value` the entry value. The other label links are reads.
pub(crate) fn parse_branch_label(
    source: &StatementSource,
    constructed: Option<TypeRef>,
    value: Expr,
) -> ParsedSource {
    let mut out = ParsedSource::default();
    let Some(field) = source.links.first() else {
        return out;
    };
    let span = TextSpan {
        start: 0,
        end: source.text.len(),
    };
    let id = statement_id(&source.id, "init", span);
    out.statements.push(Statement {
        id: id.clone(),
        source_id: source.id.clone(),
        span,
        kind: StatementKind::Init {
            constructed,
            entries: vec![InitEntry {
                field: Hop::Field {
                    link_id: field.id.clone(),
                    target: field.target.clone(),
                    visible_text: field.visible_text.clone(),
                },
                value,
            }],
            form: InitForm::DlEntries,
        },
        parent: None,
    });
    out.link_roles = (0..source.links.len())
        .map(|index| {
            let class = if index == 0 {
                OccurrenceClass::Init
            } else {
                OccurrenceClass::Read
            };
            (index, (class, id.clone()))
        })
        .collect();
    out
}

/// Statement productions: `try_*`, `push*`, `role`, `target_roles`, `read_roles`.
/// Grammar helpers live in `grammar.rs` as a second `impl Parser<'_>` block.
impl Parser<'_> {
    /// A clause starting with a link to an `INFRA_OPS` anchor: `⟦L⟧ OPERAND
    /// PREP PATH`, or `⟦L⟧ PATH` for an operation without prepositions
    /// (`map-set` takes `PATH[key] to VALUE`, `list-extend` an optional
    /// ` with VALUE`).
    fn try_infra_mutation(&mut self, at: usize) -> bool {
        let Some((link, after_link, op, prepositions)) = self.infra_op(at) else {
            return false;
        };
        let anchor = self.source.links[link].target.clone().expect("infra_op");
        let basis = OpBasis::InfraLink(anchor);
        let parsed = self.keyword(after_link, &[" "]).and_then(|start| {
            if prepositions.is_empty() {
                self.direct_infra_target(start, op)
            } else {
                self.infra_operand_target(start, prepositions)
            }
        });
        let Some((target, operand, end)) = parsed else {
            let verb = self.source.links[link].visible_text.to_lowercase();
            self.push_opaque(at, OpaqueReason::UnparsedTarget, Some(verb));
            return false;
        };
        let kind = StatementKind::Mutate {
            op,
            target: target.path.clone(),
            operand: operand.map(|(start, end)| self.expr_at(start, end)),
            basis,
        };
        let id = self.push(at, end, "mutate", kind);
        self.target_roles(std::slice::from_ref(&target), &id);
        if let Some((start, end)) = operand {
            self.read_roles(start, end, &id);
        }
        self.covered_until = end;
        true
    }

    /// The link at `at` when it targets an `INFRA_OPS` anchor: its index, the
    /// position after it, the operation and its prepositions.
    pub(crate) fn infra_op(
        &self,
        at: usize,
    ) -> Option<(usize, usize, MutationOp, &'static [&'static str])> {
        let (Placeholder::Link(link), after_link) = self.enc.placeholder(at)? else {
            return None;
        };
        let anchor = self.source.links[link]
            .target
            .as_ref()
            .filter(|target| target.spec.eq_ignore_ascii_case("INFRA"))?;
        let &(_, op, prepositions) = INFRA_OPS.iter().find(|(name, ..)| *name == anchor.anchor)?;
        Some((link, after_link, op, prepositions))
    }

    /// The target `PATH` at `start`, with the operand range after it for
    /// `map-set` and `list-extend`, and the statement end.
    fn direct_infra_target(&self, start: usize, op: MutationOp) -> Option<InfraTarget> {
        let target = self.path(start, false)?;
        let trailing = match op {
            MutationOp::MapSet if target.path.subscript.is_some() => {
                Some(self.keyword(target.end, &[" to "])?)
            }
            MutationOp::MapSet => return None,
            MutationOp::Extend => self.keyword(target.end, &[" with "]),
            _ => None,
        };
        match trailing {
            Some(value_start) => {
                let value_end = self.value_end(value_start, None);
                Some((target, Some((value_start, value_end)), value_end))
            }
            None if self.is_target_end(target.end) => {
                let end = target.end;
                Some((target, None, end))
            }
            None => None,
        }
    }

    /// `OPERAND PREP PATH` at `start`: the first preposition occurrence
    /// inside the clause whose following `PATH` ends at a target end.
    fn infra_operand_target(&self, start: usize, prepositions: &[&str]) -> Option<InfraTarget> {
        let bound = self.value_end(start, None);
        let mut candidates: Vec<(usize, usize)> = prepositions
            .iter()
            .flat_map(|prep| {
                self.enc.text[start..bound]
                    .match_indices(prep)
                    .map(move |(offset, _)| (start + offset, start + offset + prep.len()))
            })
            .filter(|&(at, _)| at > start && !self.enc.is_protected(at))
            .collect();
        candidates.sort_unstable();
        candidates
            .into_iter()
            .find_map(|(operand_end, target_start)| {
                let target = self
                    .path(target_start, false)
                    .filter(|target| self.is_target_end(target.end))?;
                let end = target.end;
                Some((target, Some((start, operand_end)), end))
            })
    }

    /// Where an Infra-linked mutation's target ends (§7.3).
    fn is_target_end(&self, pos: usize) -> bool {
        pos == self.enc.text.len()
            || [
                " with ", " at ", " before ", " after ", " if ", ", ", ".", ";", " and ",
            ]
            .iter()
            .any(|end| self.lit(pos, end))
    }

    fn try_set(&mut self, at: usize) -> bool {
        let Some(after) = self.keyword(at, &["Set ", "set "]) else {
            return false;
        };
        if let Some((mut targets, end)) = self.targets(after, true, |p, end| p.lit(end, " to ")) {
            let value_start = end + " to ".len();
            if self.is_invocation(value_start) {
                if self.try_set_to_be(at, &targets, value_start) {
                    return true;
                }
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
            let mut basis = targets.pop().expect("targets is never empty");
            while let Some(chained) = self.chained_continuation(end, &basis) {
                let value_start = chained.path.end + " to ".len();
                let targets = std::slice::from_ref(&chained.path);
                end = self.push_assignment(chained.start, targets, value_start, SetForm::Chained);
                basis = chained.path;
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

    /// `Set PATH to be ⟦L⟧ …` with a callable `⟦L⟧`: a `SetToBe` call
    /// statement whose receiver is the target, which it reads, not writes.
    fn try_set_to_be(&mut self, at: usize, targets: &[PathParse], value_start: usize) -> bool {
        let [target] = targets else {
            return false;
        };
        let pos = self.keyword(value_start, &["be "]).unwrap_or(value_start);
        let Some((Placeholder::Link(link), _)) = self.enc.placeholder(pos) else {
            return false;
        };
        if self.link_callability(link) == Callability::No {
            return false;
        }
        let end = self.value_end(value_start, None);
        let receiver = Some(Expr::Path(target.path.clone()));
        let Some(call) = self.call_at(link, CallForm::SetToBe, receiver, end) else {
            return false;
        };
        let id = self.push_call_statement(at, end, call);
        for link in target
            .root_link
            .iter()
            .chain(target.hop_links.iter().flatten())
        {
            self.role(*link, OccurrenceClass::ReadPath, &id);
        }
        for &(start, end) in &target.read_ranges {
            self.read_roles(start, end, &id);
        }
        self.covered_until = end;
        true
    }

    /// A clause that is a call (§8.5 statement position): after an optional
    /// `⌛ ` and `Optionally`, a callable link, or `Run`/`Perform`/`Invoke`/
    /// `Call`/`Potentially` and an optional `the ` before it. Its region runs
    /// to the clause end.
    fn try_call_statement(&mut self, at: usize) -> bool {
        const HEADS: [&str; 10] = [
            "Run ",
            "run ",
            "Perform ",
            "perform ",
            "Invoke ",
            "invoke ",
            "Call ",
            "call ",
            "Potentially ",
            "potentially ",
        ];
        let pos = self.keyword(at, &["\u{231B} "]).unwrap_or(at);
        let mut pos = self
            .keyword(pos, &["Optionally, ", "Optionally "])
            .unwrap_or(pos);
        if let Some(after) = self.keyword(pos, &HEADS) {
            pos = self.keyword(after, &["the "]).unwrap_or(after);
        }
        let Some((Placeholder::Link(link), after)) = self.enc.placeholder(pos) else {
            return false;
        };
        if self.infra_op(pos).is_some() || self.link_callability(link) == Callability::No {
            return false;
        }
        let end = self.value_end(after, None);
        let Some(call) = self.call_at(link, CallForm::Imperative, None, end) else {
            return false;
        };
        self.push_call_statement(at, end, call);
        self.covered_until = end;
        true
    }

    /// A `Call` statement over `start..end` for call `call`, which it owns.
    fn push_call_statement(&mut self, start: usize, end: usize, call: String) -> String {
        let id = self.push(
            start,
            end,
            "call",
            StatementKind::Call { call: call.clone() },
        );
        if let Some(call) = self.out.calls.iter_mut().find(|c| c.id == call) {
            call.statement_id = id.clone();
        }
        id
    }

    /// Every `the result of … ⟦L⟧` inside a statement whose callable link is
    /// no callee yet becomes a `ResultOf` call running to the clause end.
    fn scan_result_of_calls(&mut self) {
        let bytes = self.enc.text.as_bytes();
        let starts: Vec<usize> = self
            .enc
            .text
            .match_indices("the result of ")
            .map(|(at, _)| at)
            .filter(|&at| {
                (at == 0 || !bytes[at - 1].is_ascii_alphanumeric()) && !self.enc.is_protected(at)
            })
            .collect();
        for start in starts {
            let Some((link, after)) = self.result_of_link(start) else {
                continue;
            };
            let callee = &self.source.links[link];
            let id = call_id(&self.source.id, &callee.id);
            let span = callee.span;
            let in_statement = self
                .out
                .statements
                .iter()
                .any(|s| s.span.start <= span.start && span.end <= s.span.end);
            if !in_statement
                || self.out.calls.iter().any(|c| c.id == id)
                || self.link_callability(link) == Callability::No
            {
                continue;
            }
            let end = self.value_end(after, None);
            self.call_at(link, CallForm::ResultOf, None, end);
        }
    }

    /// Gives each call without a statement the innermost statement containing
    /// it, each call its parent (the call with the smallest region containing
    /// it) and each parent its children in span order.
    fn finish_calls(&mut self) {
        let contains =
            |outer: TextSpan, inner: TextSpan| outer.start <= inner.start && inner.end <= outer.end;
        let statements = &self.out.statements;
        let calls = &mut self.out.calls;
        for call in calls.iter_mut().filter(|c| c.statement_id.is_empty()) {
            if let Some(statement) = statements
                .iter()
                .filter(|s| contains(s.span, call.span))
                .min_by_key(|s| s.span.end - s.span.start)
            {
                call.statement_id = statement.id.clone();
            }
        }
        let parents: Vec<Option<String>> = calls
            .iter()
            .map(|call| {
                calls
                    .iter()
                    .filter(|other| {
                        other.id != call.id
                            && other.region != call.span
                            && contains(other.region, call.span)
                    })
                    .min_by_key(|other| other.region.end - other.region.start)
                    .map(|parent| parent.id.clone())
            })
            .collect();
        let mut children: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
        for (call, parent) in calls.iter_mut().zip(parents) {
            if let Some(parent) = &parent {
                children
                    .entry(parent.clone())
                    .or_default()
                    .push((call.span.start, call.id.clone()));
            }
            call.parent_call = parent;
        }
        for call in calls.iter_mut() {
            if let Some(mut nested) = children.remove(&call.id) {
                nested.sort();
                call.nested = nested.into_iter().map(|(_, id)| id).collect();
            }
        }
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
        let value_end = self.statement_value_end(value_start, targets.last());
        let value = self.expr_at(value_start, value_end);
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
                    .is_none_or(|to| to >= p.value_end(end, None))
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
            let value_end = self.value_end(value_start, None);
            (Some((value_start, value_end)), value_end)
        } else if self.is_end(target.end) {
            (None, target.end)
        } else {
            return false;
        };
        let kind = StatementKind::Mutate {
            op,
            target: target.path.clone(),
            operand: operand.map(|(start, end)| self.expr_at(start, end)),
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
        if let Some(next) = self.keyword(pos, &[" and "]) {
            if let Some((Placeholder::Var(second), end)) = self.enc.placeholder(next) {
                vars.push(second);
                pos = end;
            }
        }
        let Some(value_start) = self.keyword(pos, &[" be "]) else {
            return false;
        };
        let value_end = self.statement_value_end(value_start, None);
        let value = self.expr_at(value_start, value_end);
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
        let value_end = self.statement_value_end(value_start, None);
        let kind = StatementKind::Set {
            targets: vec![target.path.clone()],
            value: self.expr_at(value_start, value_end),
            form: SetForm::Passive,
        };
        let id = self.push(at, value_end, "set", kind);
        self.target_roles(std::slice::from_ref(&target), &id);
        self.read_roles(value_start, value_end, &id);
        self.covered_until = value_end;
        true
    }

    /// Each `a new NEWTYPE … whose …` or `a new NEWTYPE …, with …`
    /// initializer anywhere in the source becomes an `Init` statement; it
    /// consumes no clause. Returns the encoded position of each
    /// initializer's `a new` → its statement id.
    fn parse_initializers(&mut self) -> BTreeMap<usize, String> {
        let enc = self.enc;
        let bytes = enc.text.as_bytes();
        let starts: Vec<usize> = enc
            .text
            .match_indices("new ")
            .filter_map(|(new, _)| {
                let before = &enc.text[..new];
                let article = ["a ", "an "]
                    .into_iter()
                    .find(|article| before.ends_with(article))?;
                let start = new - article.len();
                let at_word = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
                (at_word && !enc.is_protected(start)).then_some(start)
            })
            .collect();
        let mut inits = BTreeMap::new();
        for start in starts {
            if let Some(id) = self.initializer(start) {
                inits.insert(start, id);
            }
        }
        inits
    }

    /// The initializer whose `a new` is at `start`, pushed as an `Init`.
    fn initializer(&mut self, start: usize) -> Option<String> {
        let type_start = self.keyword(start, &["a new ", "an new "])?;
        let (constructed, type_end) = match self.enc.placeholder(type_start) {
            Some((Placeholder::Link(_), end)) => (self.new_type(type_start), end),
            _ => (Some(self.new_type(type_start)?), self.code(type_start)?.1),
        };
        let type_end = [" node", " object", " element"]
            .iter()
            .find(|word| self.lit(type_end, word) && !self.word_continues(type_end + word.len()))
            .map_or(type_end, |word| type_end + word.len());
        let bound = self.value_end(type_end, None);
        let marker = [" whose ", ", with "]
            .iter()
            .filter_map(|word| Some((self.find(type_end, word)?, *word)))
            .filter(|&(at, _)| at < bound)
            .min()?;
        if self.enc.text[type_end..marker.0].contains("a new ") {
            return None;
        }
        let whose = marker.1 == " whose ";
        let mut pos = marker.0 + marker.1.len();
        let mut entries = Vec::new();
        let mut first_its = false;
        while let Some((hop, link, value_start, its)) = self.init_entry(pos, whose) {
            first_its |= entries.is_empty() && its;
            let value_end = self.init_value_end(value_start, bound, whose);
            entries.push((hop, link, value_start, value_end));
            match self.keyword(value_end, &[", and ", ", ", " and "]) {
                Some(next) => pos = next,
                None => break,
            }
        }
        let end = entries.last()?.3;
        let form = match (whose, first_its) {
            (true, _) => InitForm::WhoseList,
            (false, true) => InitForm::WithItsSetTo,
            (false, false) => InitForm::WithList,
        };
        let kind = StatementKind::Init {
            constructed,
            entries: entries
                .iter()
                .map(|(hop, _, value_start, value_end)| InitEntry {
                    field: hop.clone(),
                    value: self.expr_at(*value_start, *value_end),
                })
                .collect(),
            form,
        };
        let id = self.push(start, end, "init", kind);
        for (_, link, value_start, value_end) in entries {
            self.role(link, OccurrenceClass::Init, &id);
            self.read_roles(value_start, value_end, &id);
        }
        Some(id)
    }

    /// An initializer entry at `pos`: `⟦F⟧ is ` (`whose`) or `(its )?⟦F⟧ set
    /// to `. Returns the field hop, its link, the value start and whether
    /// the entry says `its`.
    fn init_entry(&self, pos: usize, whose: bool) -> Option<(Hop, usize, usize, bool)> {
        let (its, at) = match self.keyword(pos, &["its "]) {
            Some(at) if !whose => (true, at),
            _ => (false, pos),
        };
        let (hop, Some(link), end) = self.hop(at)? else {
            return None;
        };
        let value_start = self.keyword(end, &[if whose { " is " } else { " set to " }])?;
        Some((hop, link, value_start, its))
    }

    /// End of an initializer entry value: the next entry, ` to ` after a
    /// `whose` value ("… whose F is v to L"), or `bound`.
    fn init_value_end(&self, start: usize, bound: usize, whose: bool) -> usize {
        (start..bound)
            .filter(|&i| self.enc.text.is_char_boundary(i) && !self.enc.is_protected(i))
            .find(|&i| {
                (whose && self.lit(i, " to "))
                    || self
                        .keyword(i, &[", and ", ", ", " and "])
                        .is_some_and(|next| self.init_entry(next, whose).is_some())
            })
            .unwrap_or(bound)
    }

    /// An `Opaque` statement for the clause at `at`. `target_text` is the
    /// text after the verb up to ` to ` or the value end, links elided. The
    /// verb is a word or, for an Infra-linked operation, the link at `at`.
    fn push_opaque(&mut self, at: usize, reason: OpaqueReason, verb: Option<String>) -> String {
        let after = match (&verb, self.enc.placeholder(at)) {
            (Some(_), Some((Placeholder::Link(_), end))) => end + usize::from(self.lit(end, " ")),
            (Some(verb), _) => {
                let end = at + verb.len();
                end + usize::from(self.lit(end, " "))
            }
            (None, _) => at,
        };
        let end = self.value_end(after, None);
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

    /// A sentence start ends the inline block it is in. One that is no
    /// clause start may still open an `Otherwise` block ("…, then X.
    /// Otherwise, Y."). Other heads at sentence starts are no clauses: the
    /// `If`s there measured 3 of 98 conditions parsed on HTML.
    fn sentence_boundary(&mut self, sentence: usize, is_clause: bool) {
        self.inline_parent = None;
        if !is_clause && sentence >= self.covered_until && self.try_otherwise(sentence, sentence) {
            self.out.clauses.push(Clause {
                start: self.enc.to_src(sentence),
                verb: None,
                consumed: true,
            });
        }
    }

    /// Each `Init` takes the inline block of the innermost other statement
    /// containing it, the one whose clause holds the `a new`.
    fn inherit_inline_blocks(&mut self) {
        let statements = &self.out.statements;
        let parents: Vec<(usize, StatementParent)> = statements
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.kind, StatementKind::Init { .. }) && s.parent.is_none())
            .filter_map(|(index, init)| {
                let owner = statements
                    .iter()
                    .filter(|s| {
                        !matches!(s.kind, StatementKind::Init { .. })
                            && s.span.start <= init.span.start
                            && init.span.end <= s.span.end
                    })
                    .min_by_key(|s| s.span.end - s.span.start)?;
                Some((index, owner.parent.clone()?))
            })
            .collect();
        for (index, parent) in parents {
            self.out.statements[index].parent = Some(parent);
        }
    }

    /// Pushes a statement into the current inline block.
    pub(crate) fn push(
        &mut self,
        start: usize,
        end: usize,
        kind_name: &str,
        kind: StatementKind,
    ) -> String {
        let span = self.enc.span(start, end);
        let id = statement_id(&self.source.id, kind_name, span);
        self.out.statements.push(Statement {
            id: id.clone(),
            source_id: self.source.id.clone(),
            span,
            kind,
            parent: self.inline_parent.clone(),
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

#[cfg(test)]
pub(crate) mod tests_support {
    use super::StatementSource;
    use crate::parse::steps::extract_step_structure;

    /// Wrap step bodies in an algorithm, extract structure, return the sources of all segments.
    pub(crate) fn sources(steps: &[&str]) -> Vec<StatementSource> {
        let lis: String = steps
            .iter()
            .map(|s| format!("<li><p>{s}</p></li>"))
            .collect();
        let html = format!(
            r#"<div class="algorithm"><p>To <dfn id="algo">algo</dfn>:</p><ol>{lis}</ol></div>"#
        );
        sources_in(&html, "HTML")
    }

    /// Extract the structure of a whole HTML document as `spec`, return the sources of all segments.
    pub(crate) fn sources_in(html: &str, spec: &str) -> Vec<StatementSource> {
        let structure =
            extract_step_structure(html, spec, "https://html.spec.whatwg.org/", "hash:t");
        crate::state::extract::algorithm_sources(&structure).0
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::sources;
    use super::*;
    use crate::state::model::TypeKey;

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
    fn flag_target_list_repeating_the_root() {
        let (s, p) = one(
            r##"Set <a href="https://webidl.spec.whatwg.org/#this">this</a>’s <a href="#spf">stop propagation flag</a> and <a href="https://webidl.spec.whatwg.org/#this">this</a>’s <a href="#sipf">stop immediate propagation flag</a>."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Set { targets, form: SetForm::Flag, .. } if targets.len() == 2)
        );
        assert_eq!(
            role_of(&s, &p, "stop immediate propagation flag"),
            Some(OccurrenceClass::Write)
        );
        let (_, p) = one(
            r##"Set <var>e</var>’s <a href="#spf">stop propagation flag</a> and <var>x</var>’s <a href="#sipf">stop immediate propagation flag</a>."##,
        );
        assert!(!matches!(
            &p.statements[0].kind,
            StatementKind::Set { targets, .. } if targets.len() == 2
        ));
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
    fn bare_links_in_values_are_not_chained_writes() {
        for (step, visible) in [
            (
                r##"Set the <a href="#ipp">initial playback position</a> to that time and, if <var>jumped</var> is still false, <a href="#seek">seek</a> to that time."##,
                "seek",
            ),
            (
                r##"Set <var>x</var> to <var>y</var>, <a href="#clamped">clamped</a> to the range."##,
                "clamped",
            ),
            (
                r##"Set the <a href="#t">t</a> to <var>y</var> and <a href="#queue">queue a task</a> to fire."##,
                "queue a task",
            ),
        ] {
            let (s, p) = one(step);
            assert_eq!(p.statements.len(), 1, "{step}");
            assert_eq!(
                role_of(&s, &p, visible),
                Some(OccurrenceClass::Read),
                "{step}"
            );
        }
    }

    #[test]
    fn bare_link_continuations_share_the_previous_receiver() {
        let (s, p) = one(
            r##"Set <var>doctype</var>’s <a href="#n">name</a> to <var>n</var>, <a href="#p">public ID</a> to <var>p</var>, and <a href="#s">system ID</a> to <var>s</var>."##,
        );
        assert_eq!(p.statements.len(), 3);
        assert!(
            matches!(&p.statements[2].kind, StatementKind::Set { targets, form: SetForm::Chained, .. } if targets[0].root == Root::Var("doctype".into()))
        );
        for visible in ["name", "public ID", "system ID"] {
            assert_eq!(
                role_of(&s, &p, visible),
                Some(OccurrenceClass::Write),
                "{visible}"
            );
        }
    }

    #[test]
    fn bare_link_target_without_the_has_no_field_hop() {
        let (s, p) = one(r##"Set <a href="#l">l</a> to 1."##);
        assert!(
            p.statements
                .iter()
                .all(|st| !matches!(&st.kind, StatementKind::Set { targets, .. } if targets[0].root == Root::Implicit))
        );
        assert_ne!(role_of(&s, &p, "l"), Some(OccurrenceClass::Write));
    }

    #[test]
    fn of_root_does_not_cross_clause_boundaries() {
        let (s, p) = one(
            r##"Set <var>x</var> to the first item and the <a href="#len">length</a> of the list, and append it to <var>s</var>."##,
        );
        assert_ne!(role_of(&s, &p, "length"), Some(OccurrenceClass::Write));
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Set {
                form: SetForm::To,
                ..
            }
        ));
        let (s, p) = one(
            r##"Set the <a href="#sel">selectedness</a> of the first element, if any, to true."##,
        );
        assert_ne!(
            role_of(&s, &p, "selectedness"),
            Some(OccurrenceClass::Write)
        );
    }

    #[test]
    fn undefined_is_its_own_literal() {
        let (_, p) = one(r##"Set <var>x</var> to undefined."##);
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Set {
                value: Expr::Literal(Literal::Undefined),
                ..
            }
        ));
    }

    #[test]
    fn hyphenated_words_are_not_verbs() {
        let (_, p) = one(r##"Set-up steps run <var>x</var>."##);
        assert_eq!(p.clauses[0].verb, None);
        assert!(p.statements.is_empty());
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

    #[test]
    fn infra_append_to_field_and_to_local() {
        let (s, p) = one(
            r##"<a href="https://infra.spec.whatwg.org/#list-append">Append</a> <var>x</var> to <var>d</var>’s <a href="#script-blocking-style-sheet-set">script-blocking style sheet set</a>."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Mutate { op: MutationOp::Append, basis: OpBasis::InfraLink(_), operand: Some(Expr::Var(v)), .. } if v == "x")
        );
        assert_eq!(
            role_of(&s, &p, "script-blocking style sheet set"),
            Some(OccurrenceClass::Write)
        );
        let (_, p) = one(
            r##"<a href="https://infra.spec.whatwg.org/#list-append">Append</a> <var>x</var> to <var>list</var>."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Mutate { target, .. } if target.hops.is_empty() && target.root == Root::Var("list".into()))
        );
    }

    #[test]
    fn initializers_whose_and_with_its() {
        let (s, p) = one(
            r##"Return a new <a href="#concept-node">node</a> that implements <var>interface</var>, with its <a href="#concept-node-document">node document</a> set to <var>document</var>."##,
        );
        let init = p
            .statements
            .iter()
            .find(|st| matches!(st.kind, StatementKind::Init { .. }))
            .unwrap();
        assert!(
            matches!(&init.kind, StatementKind::Init { form: InitForm::WithItsSetTo, entries, .. } if entries.len() == 1)
        );
        assert_eq!(
            role_of(&s, &p, "node document"),
            Some(OccurrenceClass::Init)
        );
        let (s, p) = one(
            r##"Append a new <code><a href="https://dom.spec.whatwg.org/#text">Text</a></code> node whose <a href="https://dom.spec.whatwg.org/#concept-cd-data">data</a> is <var>text</var> and <a href="https://dom.spec.whatwg.org/#concept-node-document">node document</a> is <var>document</var> to <var>fragment</var>."##,
        );
        let init = p
            .statements
            .iter()
            .find(|st| matches!(st.kind, StatementKind::Init { .. }))
            .unwrap();
        assert!(
            matches!(&init.kind, StatementKind::Init { form: InitForm::WhoseList, entries, .. } if entries.len() == 2)
        );
        assert_eq!(
            role_of(&s, &p, "node document"),
            Some(OccurrenceClass::Init)
        );
    }

    #[test]
    fn let_with_new_points_at_its_initializer() {
        let (_, p) = one(
            r##"Let <var>t</var> be a new <a href="#tuple">thing</a> whose <a href="#f">f</a> is 1."##,
        );
        let init_id = p
            .statements
            .iter()
            .find(|st| matches!(st.kind, StatementKind::Init { .. }))
            .unwrap()
            .id
            .clone();
        assert!(p.statements.iter().any(|st| matches!(&st.kind, StatementKind::Let { value: Expr::New { init: Some(id), .. }, .. } if *id == init_id)));
    }

    #[test]
    fn initializer_types_values_and_with_list() {
        let (s, p) = one(
            r##"Append a new <a href="https://dom.spec.whatwg.org/#text">Text</a> node whose <a href="#d">data</a> is <var>text</var> to <var>fragment</var>."##,
        );
        let init = p
            .statements
            .iter()
            .find(|st| matches!(st.kind, StatementKind::Init { .. }))
            .unwrap();
        assert!(
            matches!(&init.kind, StatementKind::Init { constructed: Some(TypeRef::Unresolved(t)), entries, .. } if t.spec == "DOM" && t.anchor == "text" && entries[0].value == Expr::Var("text".into()))
        );
        assert_eq!(role_of(&s, &p, "data"), Some(OccurrenceClass::Init));
        let (s, p) = one(
            r##"Let <var>n</var> be a new <code>Text</code> node, with <a href="#d">data</a> set to <var>x</var>, and <a href="#nd">node document</a> set to <var>doc</var>’s <a href="#o">owner</a>."##,
        );
        let init = p
            .statements
            .iter()
            .find(|st| matches!(st.kind, StatementKind::Init { .. }))
            .unwrap();
        assert!(
            matches!(&init.kind, StatementKind::Init { constructed: Some(TypeRef::Known(TypeKey::Idl(name))), form: InitForm::WithList, entries } if name == "Text" && entries.len() == 2)
        );
        assert_eq!(
            role_of(&s, &p, "node document"),
            Some(OccurrenceClass::Init)
        );
        assert_eq!(role_of(&s, &p, "owner"), Some(OccurrenceClass::Read));
        let (_, p) = one(r##"Let <var>n</var> be a new <a href="#t">thing</a>."##);
        assert!(!p
            .statements
            .iter()
            .any(|st| matches!(st.kind, StatementKind::Init { .. })));
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Let {
                value: Expr::New {
                    init: None,
                    ty: Some(_)
                },
                ..
            }
        ));
    }

    #[test]
    fn infra_prepositions_map_set_and_unparsed_targets() {
        let (s, p) = one(
            r##"<a href="https://infra.spec.whatwg.org/#list-remove">Remove</a> <var>x</var> from the <a href="#q">pending queue</a>, then return."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Mutate { op: MutationOp::Remove, target, .. } if target.root == Root::Implicit)
        );
        assert_eq!(
            role_of(&s, &p, "pending queue"),
            Some(OccurrenceClass::Write)
        );
        assert!(p.clauses[0].consumed);
        let (s, p) = one(
            r##"<a href="https://infra.spec.whatwg.org/#map-set">Set</a> <var>d</var>’s <a href="#m">map</a>[<var>k</var>] to <var>e</var>’s <a href="#v">value</a>."##,
        );
        assert!(
            matches!(&p.statements[0].kind, StatementKind::Mutate { op: MutationOp::MapSet, target, operand: Some(Expr::Path(_)), .. } if target.subscript.is_some())
        );
        assert_eq!(role_of(&s, &p, "map"), Some(OccurrenceClass::Write));
        assert_eq!(role_of(&s, &p, "value"), Some(OccurrenceClass::Read));
        let (s, p) = one(
            r##"<a href="https://infra.spec.whatwg.org/#list-empty">Empty</a> <var>d</var>’s <a href="#l">list</a>."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Mutate {
                op: MutationOp::Empty,
                operand: None,
                ..
            }
        ));
        assert_eq!(role_of(&s, &p, "list"), Some(OccurrenceClass::Write));
        let (s, p) = one(
            r##"<a href="https://infra.spec.whatwg.org/#list-append">Append</a> <var>x</var> to the end of the <a href="#l">list</a>."##,
        );
        assert!(matches!(
            &p.statements[0].kind,
            StatementKind::Opaque { reason: OpaqueReason::UnparsedTarget, verb: Some(v), .. } if v == "append"
        ));
        assert_eq!(role_of(&s, &p, "list"), None);
        assert_eq!(p.statements.len(), 1);
        assert!(!p.clauses[0].consumed);
        assert_eq!(p.clauses[0].verb.as_deref(), Some("append"));
    }

    #[test]
    fn initializer_scan_steps_over_multibyte_text() {
        for step in [
            r##"Let <var>t</var> be <var>x</var> new <a href="#t">thing</a>."##,
            r##"Let <var>t</var> be <a href="#x">x</a> new <a href="#t">thing</a>."##,
            r##"Let <var>t</var> be «a new <a href="#t">thing</a> whose <a href="#f">f</a> is 1»."##,
        ] {
            let (_, p) = one(step);
            assert!(p.clauses[0].consumed, "{step}");
        }
        let (s, p) = one(
            r##"Let <var>t</var> be é new «a new <a href="#t">thing</a> whose <a href="#f">f</a> is 1»."##,
        );
        assert_eq!(role_of(&s, &p, "f"), Some(OccurrenceClass::Init));
    }

    #[test]
    fn value_ends_before_a_following_infra_clause() {
        let (s, p) = one(
            r##"Set <var>x</var>’s <a href="#a">a</a> to <var>y</var>, and <a href="https://infra.spec.whatwg.org/#list-append">append</a> <var>z</var> to <var>w</var>’s <a href="#b">b</a>."##,
        );
        assert_eq!(p.statements.len(), 2);
        assert!(matches!(
            &p.statements[1].kind,
            StatementKind::Mutate {
                op: MutationOp::Append,
                ..
            }
        ));
        assert_eq!(role_of(&s, &p, "b"), Some(OccurrenceClass::Write));
    }

    #[test]
    fn new_statement_kinds_and_parent_round_trip() {
        let statement = Statement {
            id: "stmt-1".into(),
            source_id: "src-1".into(),
            span: TextSpan { start: 0, end: 7 },
            kind: StatementKind::Return { value: None },
            parent: Some(StatementParent {
                statement_id: "stmt-0".into(),
                role: BlockRole::Then,
            }),
        };
        let json = serde_json::to_value(&statement).unwrap();
        assert_eq!(json["kind"], serde_json::json!({"return": {"value": null}}));
        assert_eq!(json["parent"]["role"], "then");
        assert_eq!(
            serde_json::from_value::<Statement>(json).unwrap(),
            statement
        );
        let old = serde_json::json!({"id": "s", "source_id": "x", "span": {"start": 0, "end": 1}, "kind": {"opaque": {"reason": "other", "verb": null, "target_text": null}}});
        assert_eq!(
            serde_json::from_value::<Statement>(old).unwrap().parent,
            None
        );
    }

    #[test]
    fn predicate_and_call_serialization_shapes() {
        let p = Predicate::Is {
            operand: Expr::Var("x".into()),
            test: Test::Null,
            negated: true,
        };
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({"is": {"operand": {"var": "x"}, "test": "null", "negated": true}})
        );
        assert_eq!(
            serde_json::to_value(RunContext::InParallel).unwrap(),
            serde_json::json!("in_parallel")
        );
        assert_eq!(
            serde_json::to_value(CallForm::SetToBe).unwrap(),
            serde_json::json!("set_to_be")
        );
        assert_eq!(
            serde_json::to_value(LinkRole::Callee {
                call: "call-1".into()
            })
            .unwrap(),
            serde_json::json!({"callee": {"call": "call-1"}})
        );
        assert_eq!(
            serde_json::to_value(Origin::Param { index: 0 }).unwrap(),
            serde_json::json!({"param": {"index": 0}})
        );
    }

    #[test]
    fn call_ids_are_stable_and_distinct() {
        assert_eq!(call_id("src-a", "link-1"), call_id("src-a", "link-1"));
        assert_ne!(call_id("src-a", "link-1"), call_id("src-a", "link-2"));
        assert!(call_id("src-a", "link-1").starts_with("call-"));
    }

    /// SP1's statements and occurrence roles for one multi-form step list, pinned
    /// before the grammar moves to grammar.rs. Update the literal only when a later
    /// task changes SP1 output on purpose, and say why in its commit body.
    #[test]
    fn sp1_parse_of_a_multi_form_fixture_is_pinned() {
        let steps = [
            "Let <var>x</var> be <var>d</var>'s <a href=\"#f\">f</a>.",
            "Set <var>d</var>'s <a href=\"#f\">f</a> to true.",
            "Set <var>a</var>'s <a href=\"#g\">g</a> and <var>b</var>'s <a href=\"#g\">g</a> to null.",
            "<a href=\"https://infra.spec.whatwg.org/#list-append\">Append</a> <var>x</var> to <var>d</var>'s <a href=\"#h\">h</a>.",
            "Let <var>e</var> be a new <a href=\"#event\">event</a> whose <a href=\"#type\">type</a> is <code>load</code>.",
            "Set <var>d</var>'s <a href=\"#f\">f</a> to the result of running <a href=\"#run\">run</a> given <var>x</var>.",
        ];
        let got: Vec<String> = sources(&steps)
            .iter()
            .map(|src| {
                let p = parse_source(src);
                format!(
                    "{:?} | {:?}",
                    p.statements
                        .iter()
                        .map(|s| (&s.kind, s.span))
                        .collect::<Vec<_>>(),
                    p.link_roles
                )
            })
            .collect();
        let want: Vec<&str> = vec![
            r##"[(Let { var: "x", value: Path(Path { root: Var("d"), hops: [Field { link_id: "src-2314753dcbff666aba41857592bbaf409ba535b2d73d3afa8eab32d38b36c934", target: Some(AnchorTarget { spec: "HTML", anchor: "f" }), visible_text: "f" }], subscript: None }) }, TextSpan { start: 0, end: 18 })] | {0: (Read, "stmt-a93309e1f9134971e60692c1013e33eb3743f5315e250cfb1aa66adf77f48796")}"##,
            r##"[(Set { targets: [Path { root: Var("d"), hops: [Field { link_id: "src-8479fb6fb8287e762d07630bb01259c7ae64f8e1d0df9363f59f5276dee0aea4", target: Some(AnchorTarget { spec: "HTML", anchor: "f" }), visible_text: "f" }], subscript: None }], value: Literal(Bool(true)), form: To }, TextSpan { start: 0, end: 19 })] | {0: (Write, "stmt-7aa9b61f42bc96f5868a16cdc18e52c63f9ac46bc19728fc8ab137387069e701")}"##,
            r##"[(Opaque { reason: UnparsedTarget, verb: Some("set"), target_text: Some("*a*'s _ and *b*'s _") }, TextSpan { start: 0, end: 31 })] | {}"##,
            r##"[(Mutate { op: Append, target: Path { root: Var("d"), hops: [Field { link_id: "src-f4e72ec47b9aa1ff3324e43f10524bc7e5be3d31cd04f38799e211692c2e3963", target: Some(AnchorTarget { spec: "HTML", anchor: "h" }), visible_text: "h" }], subscript: None }, operand: Some(Var("x")), basis: InfraLink(AnchorTarget { spec: "INFRA", anchor: "list-append" }) }, TextSpan { start: 0, end: 21 })] | {1: (Write, "stmt-a42f9d814b6054e1b18cc295eb44904d817f1f3af37a1bdc6e1a9acc44bbff3b")}"##,
            r##"[(Init { constructed: Some(Unresolved(AnchorTarget { spec: "HTML", anchor: "event" })), entries: [InitEntry { field: Field { link_id: "src-e512ac427d9e15dc6b48b91bb630b159462b568ff1f259a6667e6c7c258ab347", target: Some(AnchorTarget { spec: "HTML", anchor: "type" }), visible_text: "type" }, value: EnumValue { text: "load", target: None } }], form: WhoseList }, TextSpan { start: 11, end: 43 }), (Let { var: "e", value: New { ty: Some(Unresolved(AnchorTarget { spec: "HTML", anchor: "event" })), init: Some("stmt-435756353184c4234abc1bea20f0e2715f4d28220955a92844a51230203a88e8") } }, TextSpan { start: 0, end: 43 })] | {0: (Read, "stmt-64b9ac000c9c7b93deb580c9682dbe3afbdf7589eeceb35dd873728b23e081de"), 1: (Init, "stmt-435756353184c4234abc1bea20f0e2715f4d28220955a92844a51230203a88e8")}"##,
            r##"[(Set { targets: [Path { root: Var("d"), hops: [Field { link_id: "src-4c9d44d6f62ce27f89265357f0c781fb468c341c66d6dbf5973b154e97360921", target: Some(AnchorTarget { spec: "HTML", anchor: "f" }), visible_text: "f" }], subscript: None }], value: Opaque { text: "the result of running run given *x*" }, form: To }, TextSpan { start: 0, end: 50 })] | {0: (Write, "stmt-0d778edcb455a9939e1829e5acd63340da295b8858e4f7018ce958447592b0e3"), 1: (Read, "stmt-0d778edcb455a9939e1829e5acd63340da295b8858e4f7018ce958447592b0e3")}"##,
        ];
        assert_eq!(got, want);
    }

    mod calls {
        use crate::state::grammar::{Callable, Env};
        use crate::state::ir::tests_support::{sources, sources_in};
        use crate::state::ir::*;
        use crate::state::model::Literal;
        use crate::state::testing::var;

        fn env(spec: &str, callables: &[(&str, Callable)]) -> Env {
            Env {
                spec: spec.into(),
                callables: callables.iter().map(|(a, c)| (a.to_string(), *c)).collect(),
                ..Env::default()
            }
        }
        fn parse(step: &str, env: &Env) -> (StatementSource, ParsedSource) {
            let src = sources(&[step]).remove(0);
            let parsed = parse_source_with(&src, env);
            (src, parsed)
        }

        #[test]
        fn imperative_and_run_heads() {
            let e = env(
                "HTML",
                &[("navigate", Callable::Template), ("x", Callable::Template)],
            );
            let (_, p) = parse(
                "<a href=\"#navigate\">Navigate</a> <var>n</var> to <var>u</var>.",
                &e,
            );
            assert!(
                matches!(&p.statements[0].kind, StatementKind::Call { call } if call == &p.calls[0].id)
            );
            assert_eq!(
                (p.calls[0].form, p.calls[0].statement_id.clone()),
                (CallForm::Imperative, p.statements[0].id.clone())
            );
            let (_, p) = parse("Run <a href=\"#x\">x</a> given <var>y</var>.", &e);
            assert_eq!(p.calls[0].form, CallForm::Imperative);
        }

        #[test]
        fn result_of_callx_gerund_and_algorithm_values() {
            let e = env(
                "HTML",
                &[("x", Callable::Template), ("fire", Callable::Template)],
            );
            let (_, p) = parse(
                "Let <var>r</var> be the result of running <a href=\"#x\">x</a> given <var>y</var>.",
                &e,
            );
            assert!(
                matches!(&p.statements[0].kind, StatementKind::Let { value: Expr::Call(id), .. } if id == &p.calls[0].id)
            );
            assert_eq!(p.calls[0].form, CallForm::ResultOf);
            let (_, p) = parse("Let <var>d</var> be <a href=\"https://url.spec.whatwg.org/#concept-url-parser\">URL parser</a> given <var>s</var>.", &e);
            assert_eq!(p.calls[0].form, CallForm::ResultOf);
            let (_, p) = parse("Let <var>d</var> be <a href=\"https://dom.spec.whatwg.org/#concept-node-document\">node document</a> of <var>n</var>.", &e);
            assert!(p.calls.is_empty());
            let (_, p) = parse(
                "Let <var>r</var> be <a href=\"#fire\">firing an event</a> named <code>x</code> at <var>t</var>.",
                &e,
            );
            assert!(
                matches!(&p.statements[0].kind, StatementKind::Let { value: Expr::Call(id), .. } if id == &p.calls[0].id)
            );
            assert_eq!(p.calls[0].form, CallForm::Gerund);
            let (_, p) = parse(
                "Set <var>request</var>'s <a href=\"#steps\">steps</a> to <a href=\"#x\">x</a>.",
                &e,
            );
            assert!(
                matches!(&p.statements[0].kind, StatementKind::Set { value: Expr::AlgorithmRef { target: Some(t), .. }, .. } if t.anchor == "x")
            );
            assert_eq!(p.roles[&1], LinkRole::AlgorithmValue);
            assert!(p.calls.is_empty());
        }

        #[test]
        fn set_to_be_possessive_and_ecmarkup() {
            let e = env(
                "HTML",
                &[
                    ("blocked", Callable::Predicate),
                    ("fallback-base-url", Callable::Accessor),
                ],
            );
            let (_, p) = parse("Set <var>subject</var>'s <a href=\"#nd\">node document</a> to be <a href=\"#blocked\">blocked by a modal dialog</a>.", &e);
            assert!(matches!(&p.statements[0].kind, StatementKind::Call { .. }));
            assert_eq!(p.calls[0].form, CallForm::SetToBe);
            assert!(matches!(&p.calls[0].receiver, Some(Expr::Path(_))));
            let (src, p) = parse(
                "Let <var>u</var> be <var>doc</var>’s <a href=\"#fallback-base-url\">fallback base URL</a>.",
                &e,
            );
            assert_eq!(
                (p.calls[0].form, p.calls[0].receiver.clone()),
                (CallForm::Possessive, Some(var("doc")))
            );
            assert_eq!(
                &src.text[p.calls[0].span.start..p.calls[0].span.end],
                "fallback base URL"
            );
            let html = crate::state::testing::ECMA_HTML;
            let src = sources_in(html, "ECMA-262")
                .into_iter()
                .find(|s| s.text.contains("StringIndexOf("))
                .unwrap();
            let p = parse_source_with(
                &src,
                &env("ECMA-262", &[("sec-stringindexof", Callable::Template)]),
            );
            assert_eq!(p.calls[0].form, CallForm::Ecmarkup);
            assert_eq!(
                &src.text[p.calls[0].region.start..p.calls[0].region.end],
                "(*s*, \"x\", 0)"
            );
        }

        #[test]
        fn nested_calls_share_the_statement_and_know_their_parent() {
            let e = env(
                "HTML",
                &[("x", Callable::Template), ("y", Callable::Template)],
            );
            let (_, p) = parse("Let <var>a</var> be the result of <a href=\"#x\">x</a> given the result of <a href=\"#y\">y</a> given <var>b</var>.", &e);
            assert_eq!(p.calls.len(), 2);
            let outer = p
                .calls
                .iter()
                .find(|c| c.callee.visible_text == "x")
                .unwrap();
            let inner = p
                .calls
                .iter()
                .find(|c| c.callee.visible_text == "y")
                .unwrap();
            assert_eq!(inner.parent_call.as_deref(), Some(outer.id.as_str()));
            assert_eq!(
                (outer.nested.clone(), inner.nested.clone()),
                (vec![inner.id.clone()], vec![])
            );
            assert_eq!(inner.statement_id, outer.statement_id);
            assert_eq!(outer.statement_id, p.statements[0].id);
        }

        #[test]
        fn set_to_be_reads_its_target_and_needs_a_callable_link() {
            let e = env("HTML", &[("blocked", Callable::Predicate)]);
            let step = "Set <var>subject</var>'s <a href=\"#nd\">node document</a> to be <a href=\"#blocked\">blocked by a modal dialog</a>.";
            let (_, p) = parse(step, &e);
            let StatementKind::Call { call } = &p.statements[0].kind else {
                panic!("{:?}", p.statements)
            };
            assert_eq!(&p.calls[0].id, call);
            assert_eq!(p.calls[0].statement_id, p.statements[0].id);
            assert_eq!(p.link_roles[&0].0, OccurrenceClass::ReadPath);
            let (_, p) = parse(step, &env("HTML", &[]));
            assert!(p.calls.is_empty());
            assert!(matches!(
                &p.statements[0].kind,
                StatementKind::Opaque {
                    reason: OpaqueReason::ValueIsInvocation,
                    ..
                }
            ));
        }

        #[test]
        fn statement_heads_skip_markers_and_exclude_infra_and_mentions() {
            let e = env("HTML", &[("x", Callable::Template), ("y", Callable::Body)]);
            let (src, p) = parse(
                "Optionally, <a href=\"#x\">x</a> <var>a</var>, then run the <a href=\"#y\">y</a> steps.",
                &e,
            );
            assert_eq!(p.calls.len(), 2);
            assert!(p.calls.iter().all(|c| c.form == CallForm::Imperative));
            assert_eq!(p.statements.len(), 2);
            assert!(p
                .statements
                .iter()
                .all(|s| matches!(s.kind, StatementKind::Call { .. })));
            let x = &p.calls[0];
            assert_eq!(&src.text[x.region.start..x.region.end], " *a*");
            let (_, p) = parse(
                "<a href=\"https://infra.spec.whatwg.org/#list-append\">Append</a> <var>a</var> to <var>b</var>.",
                &e,
            );
            assert!(p.calls.is_empty());
            let src = sources(&["<a href=\"#x\">x</a> <var>a</var>."]).remove(0);
            let SourceContext::Algorithm { segment_id, .. } = &src.context else {
                panic!()
            };
            let mut e = e.clone();
            e.mentions
                .insert((segment_id.clone(), src.links[0].id.clone()));
            let p = parse_source_with(&src, &e);
            assert!(p.calls.is_empty() && p.statements.is_empty());
        }

        #[test]
        fn of_accessor_new_type_and_fallback_scan() {
            let e = env(
                "HTML",
                &[("url", Callable::Accessor), ("x", Callable::Template)],
            );
            let (_, p) = parse(
                "Let <var>u</var> be the <a href=\"#url\">URL</a> of <var>doc</var>.",
                &e,
            );
            assert_eq!(
                (p.calls[0].form, p.calls[0].receiver.clone()),
                (CallForm::Possessive, Some(var("doc")))
            );
            let (_, p) = parse(
                "Let <var>e</var> be a new <a href=\"#event\">event</a>.",
                &e,
            );
            assert_eq!(p.roles[&0], LinkRole::Type);
            // Inside an opaque statement the scan finds the call.
            let (_, p) = parse("Set <var>a</var>'s <a href=\"#f\">f</a> and <var>b</var>'s <a href=\"#g\">g</a> to the result of <a href=\"#x\">x</a> given <var>c</var>.", &e);
            assert_eq!(p.calls.len(), 1);
            assert_eq!(p.calls[0].form, CallForm::ResultOf);
            assert_eq!(p.calls[0].statement_id, p.statements[0].id);
            // An `If` condition owns the call its predicate finds.
            let (_, p) = parse("If the result of <a href=\"#x\">x</a> is true, return.", &e);
            assert_eq!(p.calls[0].statement_id, p.statements[0].id);
            // Outside every statement it records nothing.
            let (_, p) = parse(
                "When the result of <a href=\"#x\">x</a> is true, return.",
                &e,
            );
            assert!(p.calls.is_empty());
        }

        #[test]
        fn default_env_keeps_sp1_behavior() {
            let (_, p) = parse(
                "Let <var>r</var> be the result of running <a href=\"#x\">x</a> given <var>y</var>.",
                &Env::default(),
            );
            assert!(p.calls.is_empty());
            assert!(matches!(
                &p.statements[0].kind,
                StatementKind::Let {
                    value: Expr::Opaque { .. },
                    ..
                }
            ));
            let _ = Literal::Null;
        }
    }

    fn assert_one_conditional(p: &ParsedSource) -> &Expr {
        assert!(
            p.statements
                .iter()
                .all(|s| !matches!(s.kind, StatementKind::Otherwise { .. })),
            "{:?}",
            p.statements
        );
        assert_eq!(p.statements.len(), 1, "{:?}", p.statements);
        let value = match &p.statements[0].kind {
            StatementKind::Let { value, .. } => value,
            StatementKind::Return { value: Some(value) } => value,
            other => panic!("{other:?}"),
        };
        assert!(matches!(value, Expr::Conditional { .. }), "{value:?}");
        value
    }

    #[test]
    fn conditional_let_is_one_statement() {
        let (s, p) = one("Let <var>a</var> be 1 if <var>p</var> is null; otherwise 2.");
        let Expr::Conditional {
            condition,
            then,
            otherwise,
        } = assert_one_conditional(&p)
        else {
            unreachable!()
        };
        assert!(matches!(**condition, Predicate::Is { .. }));
        assert_eq!(**then, Expr::Literal(Literal::Number("1".into())));
        assert_eq!(**otherwise, Expr::Literal(Literal::Number("2".into())));
        assert!(s.text[..p.statements[0].span.end].ends_with('2'));
    }

    #[test]
    fn conditional_return_is_one_statement() {
        let (_, p) = one("Return true if <var>p</var> is null; otherwise, false.");
        assert_one_conditional(&p);
    }

    #[test]
    fn otherwise_without_an_if_in_the_source_is_no_statement() {
        let (_, p) = one("Set <var>x</var> to <var>a</var> if <var>p</var> is null; otherwise, set <var>x</var> to <var>b</var>.");
        let [StatementKind::Set { .. }, StatementKind::Set { value, .. }] =
            &p.statements.iter().map(|s| &s.kind).collect::<Vec<_>>()[..]
        else {
            panic!("{:?}", p.statements)
        };
        assert_eq!(value, &Expr::Var("b".into()));
        let (_, p) = one("Let <var>a</var> be 1 if <var>p</var> is null; otherwise return null.");
        assert!(matches!(
            &p.statements.iter().map(|s| &s.kind).collect::<Vec<_>>()[..],
            [
                StatementKind::Let { .. },
                StatementKind::Return {
                    value: Some(Expr::Literal(Literal::Null))
                }
            ]
        ));
        let (_, p) = one("If <var>b</var> is true, then set <var>x</var> to <var>a</var> if <var>p</var> is null; otherwise, set <var>x</var> to <var>b</var>.");
        assert!(p
            .statements
            .iter()
            .any(|s| matches!(s.kind, StatementKind::Otherwise { of: Some(_), .. })));
    }

    #[test]
    fn init_takes_the_inline_block_of_its_statement() {
        let (_, p) = one(
            r##"If <var>x</var> is null, then let <var>e</var> be a new <a href="https://dom.spec.whatwg.org/#concept-event">event</a> whose <a href="https://dom.spec.whatwg.org/#dom-event-type">type</a> is "load"."##,
        );
        let if_id = &p
            .statements
            .iter()
            .find(|s| matches!(s.kind, StatementKind::If { .. }))
            .unwrap()
            .id;
        let parent_of = |pick: fn(&StatementKind) -> bool| {
            p.statements
                .iter()
                .find(|s| pick(&s.kind))
                .and_then(|s| s.parent.clone())
                .map(|q| (q.statement_id, q.role))
        };
        let then = Some((if_id.clone(), BlockRole::Then));
        assert_eq!(parent_of(|k| matches!(k, StatementKind::Let { .. })), then);
        assert_eq!(parent_of(|k| matches!(k, StatementKind::Init { .. })), then);
    }
}
