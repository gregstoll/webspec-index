//! Minimal statement IR (§7.1 sources, §7.2 statements and expressions).
//!
//! Task A9 adds the parser to this file. Until then, these are the type
//! definitions only.
use serde::{Deserialize, Serialize};

use crate::parse::steps::{AnchorTarget, InlineToken, LinkSpan, TextSpan};
use crate::state::model::{Literal, TypeRef};

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
