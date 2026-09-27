use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

pub use crate::parse::steps::{AnchorTarget, InlineToken, InlineTokenKind, LinkSpan, TextSpan};
pub use crate::state::catalog::StateCatalog;

pub const STATE_VERSION: &str = "16";

/// Canonical type identity. IDL types are keyed by IDL name, globally, so that
/// `partial interface Document` in HTML and `interface Document` in DOM are one
/// type. Every other type is keyed by its defining anchor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TypeKey {
    Idl(String),
    Anchor(AnchorTarget),
}

impl TypeKey {
    pub fn parse(value: &str) -> Option<Self> {
        if let Some(name) = value.strip_prefix("idl:") {
            return (!name.is_empty()).then(|| Self::Idl(name.to_string()));
        }
        let (spec, anchor) = value.split_once('#')?;
        (!spec.is_empty() && !anchor.is_empty()).then(|| {
            Self::Anchor(AnchorTarget {
                spec: spec.to_string(),
                anchor: anchor.to_string(),
            })
        })
    }
}

impl fmt::Display for TypeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idl(name) => write!(f, "idl:{name}"),
            Self::Anchor(target) => write!(f, "{}#{}", target.spec, target.anchor),
        }
    }
}

impl Serialize for TypeKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TypeKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        TypeKey::parse(&value)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid type key {value}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDef {
    pub key: TypeKey,
    pub name: String,
    pub kind: TypeKind,
    pub anchors: Vec<TypeAnchor>,
    pub supertypes: Vec<SuperEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeKind {
    IdlInterface,
    IdlMixin,
    IdlDictionary,
    InfraStruct,
    Concept,
    Element,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeAnchor {
    pub target: AnchorTarget,
    pub role: AnchorRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorRole {
    Defining,
    Partial,
    ConceptAlias(ConceptAliasBasis),
    ElementDefinition,
}

/// How a concept-dfn alias to an IDL type was established (§6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConceptAliasBasis {
    /// Normalized name (lowercase, spaces removed) matched an IDL interface name in the same spec.
    NameMatch,
    /// Established by an explicit link, `data-dfn-for`, or a reviewed YAML declaration.
    Evidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperEdge {
    pub target: TypeKey,
    pub basis: SuperBasis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuperBasis {
    IdlInheritance,
    IdlIncludes,
    Override { rule_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    pub anchor: AnchorTarget,
    /// Display form: `<code>` runs in backticks.
    pub name: String,
    /// Plain dfn text, then its `data-lt` alternatives.
    pub names: Vec<String>,
    pub owner: Owner,
    pub field_basis: FieldBasis,
    pub declared_type: TypeExpr,
    pub initial: Option<InitialValue>,
    pub declaration: Option<DeclarationSite>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    Known {
        types: Vec<OwnerRef>,
        basis: OwnerBasis,
    },
    Unknown {
        hint: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerRef {
    pub key: TypeKey,
    pub via: OwnerVia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerVia {
    Link,
    IdlName,
    DfnName,
    Declaration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerBasis {
    DfnFor,
    DeclarationSentence,
    PropertyList,
    StructItems,
    Override { rule_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldBasis {
    Declared,
    DfnForOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationSite {
    pub section_anchor: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeExpr {
    Unknown,
    Primitive(Primitive),
    Nominal {
        ty: TypeRef,
        text: String,
    },
    Infra {
        kind: InfraKind,
        args: Vec<TypeExpr>,
    },
    Union(Vec<TypeExpr>),
    Null,
    Enumerated(Vec<String>),
    Opaque {
        text: String,
    },
    /// An IDL type reference (e.g. `DOMString`) without a resolved anchor.
    Idl {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primitive {
    Boolean,
    String,
    Number,
    Integer,
    ByteSequence,
    ScalarValueString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InfraKind {
    List,
    OrderedSet,
    OrderedMap,
    Map,
    Tuple,
    Struct,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeRef {
    Known(TypeKey),
    Unresolved(AnchorTarget),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialValue {
    Literal { value: Literal, text: String },
    Empty { text: String },
    New { ty: TypeExpr, text: String },
    Unset { text: String },
    Opaque { text: String },
}

/// A literal value used in initial values and IR expressions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Literal {
    Bool(bool),
    Null,
    Undefined,
    Number(String),
    String(String),
    Failure,
}

/// HTML reflection: an IDL attribute whose getter and setter read and write a
/// content attribute. Derived structurally (§7.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reflection {
    pub idl_attribute: AnchorTarget,
    pub idl_attribute_name: String,
    pub content_attribute: Option<AnchorTarget>,
    pub content_attribute_name: String,
    pub basis: ReflectionBasis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionBasis {
    IdlExtendedAttribute(String),
    ProseReflect,
}

/// A flag that is a member of a set-valued concept ("A sandboxing flag set is a
/// set of zero or more of the following flags"). Members are never fields (§6.3 R5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetMember {
    pub anchor: AnchorTarget,
    pub name: String,
    pub names: Vec<String>,
    pub set: TypeKey,
    pub declaration: DeclarationSite,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectModel {
    pub types: Vec<TypeDef>,
    pub fields: Vec<FieldDef>,
    pub members: Vec<SetMember>,
    pub reflections: Vec<Reflection>,
}

// ---------------------------------------------------------------------------
// Signature types (§8)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub algorithm: AnchorTarget,
    pub source_id: String,
    pub form: SignatureForm,
    pub this: Option<TypeExpr>,
    pub params: Vec<Param>,
    pub returns: Option<ReturnType>,
    pub template: Option<Template>,
    pub issues: Vec<SignatureIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureForm {
    To,
    WhenStepsSay,
    GivenList,
    IdlMethod { interface: String, member: String },
    IdlGetter { interface: String, member: String },
    IdlSetter { interface: String, member: String },
    IdlConstructor { interface: String },
    Accessor,
    Predicate,
    Declared,
    Ecmarkup,
}

impl SignatureForm {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::To => "to",
            Self::WhenStepsSay => "when_steps_say",
            Self::GivenList => "given_list",
            Self::IdlMethod { .. } => "idl_method",
            Self::IdlGetter { .. } => "idl_getter",
            Self::IdlSetter { .. } => "idl_setter",
            Self::IdlConstructor { .. } => "idl_constructor",
            Self::Accessor => "accessor",
            Self::Predicate => "predicate",
            Self::Declared => "declared",
            Self::Ecmarkup => "ecmarkup",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub anchor: Option<AnchorTarget>,
    pub ty: TypeExpr,
    pub type_text: String,
    pub type_basis: TypeBasis,
    pub optional: bool,
    pub default: Option<crate::state::ir::Expr>,
    pub passing: Passing,
    pub span: crate::parse::steps::TextSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeBasis {
    Explicit,
    NameResolved,
    DfnFor,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Passing {
    Positional { index: u32 },
    Named,
    Receiver,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub pieces: Vec<TemplatePiece>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplatePiece {
    Head(String),
    Callee,
    Literal(String),
    Slot(u32),
    ListSep,
    NamedGroup(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnType {
    pub ty: TypeExpr,
    pub basis: ReturnBasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReturnBasis {
    Intro,
    Idl,
    Ecmarkup,
    ReturnStatements,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureIssue {
    UntypedParam(String),
    OpaqueType(String),
    UnparsedIntroTail(String),
    IdlMemberNotFound,
    DuplicateParam(String),
}

/// Algorithm-level summary for one anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlgorithmSummary {
    pub anchor: String,
    pub statements: BTreeMap<String, u32>,
    pub opaque: Vec<ReviewItem>,
    pub var_origins: Vec<crate::state::ir::VarOrigin>,
    pub calls: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceClass {
    Write,
    Init,
    ReadPath,
    Read,
    Unclassified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occurrence {
    pub source_id: String,
    pub link_id: String,
    pub target: Option<AnchorTarget>,
    pub class: OccurrenceClass,
    pub statement_id: Option<String>,
    /// `ir`, or `rule:<package>/<id>`.
    pub basis: String,
    /// Public ids of further rules that matched this write or init, which
    /// keeps the class and `basis` of whatever produced it first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteClass {
    Write,
    Init,
    Unclassified,
    OpaqueWrite,
    Declared,
}

impl SiteClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Init => "init",
            Self::Unclassified => "unclassified",
            Self::OpaqueWrite => "opaque_write",
            Self::Declared => "declared",
        }
    }
}

/// One `state_sites` row (spec §9.2, §16.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    pub site_id: String,
    pub class: SiteClass,
    pub target: Option<AnchorTarget>,
    pub op: String,
    pub subject: AnchorTarget,
    pub context: String,
    pub role: Option<String>,
    pub constructed: Option<String>,
    pub step_path: Option<String>,
    pub step_id: Option<String>,
    pub receiver: String,
    pub target_text: String,
    pub value_text: Option<String>,
    pub text: String,
    pub basis: String,
    /// Structural segment id (§16.1); NULL for prose and declared sites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<String>,
    /// Algorithm body id (§16.1); NULL for prose and declared sites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_id: Option<String>,
    /// UTF-8 byte start of the statement in the segment text (§16.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_start: Option<u32>,
    /// UTF-8 byte end of the statement in the segment text (§16.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_end: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateIssueCode {
    OwnerUnknown,
    OwnerConflict,
    FieldNotDeclared,
    AmbiguousSelector,
    AmbiguousField,
    UnresolvedLink,
    MissingSpec,
    UnclassifiedOccurrence,
    PossibleUnlinkedWrite,
    DeclarationMismatch,
    OutsideStructure,
    DanglingOtherwise,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIssue {
    pub code: StateIssueCode,
    pub anchor: Option<String>,
    pub message: String,
}

/// The §5 counters for one snapshot. Every field has `#[serde(default)]` so
/// later stages add counters without breaking stored payloads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CoverageCounters {
    pub concept_dfns: u32,
    pub owner_by_rule: BTreeMap<String, u32>,
    pub owner_candidates: u32,
    pub owner_resolved: u32,
    pub set_members: u32,
    pub written_fields: u32,
    pub written_fields_owned: BTreeMap<String, u32>,
    pub written_fields_unresolved: Vec<String>,
    pub statements: BTreeMap<String, u32>,
    pub set_total: u32,
    pub set_structured: u32,
    pub occurrences: BTreeMap<String, u32>,
    pub unresolved_links: u32,
    pub prose_sources: u32,
    pub prose_callouts_excluded: u32,
    pub prose_mentions: u32,
    pub unclassified_review: Vec<ReviewItem>,
    /// Number of structural algorithms.
    pub algorithms: u32,
    /// `SignatureForm::as_str` → count; `"none"` = algorithm without signature.
    pub intro_forms: BTreeMap<String, u32>,
    pub template_signatures: u32,
    /// `"explicit" | "name_resolved" | "dfn_for" | "unknown" | "opaque"` → count (To-like only).
    pub to_params: BTreeMap<String, u32>,
    pub idl_signatures: u32,
    /// IDL signatures without `IdlMemberNotFound`.
    pub idl_from_idl: u32,
    pub steps: u32,
    pub steps_recognized: u32,
    /// Statement kind tag of the head step, or `"unrecognized"`.
    pub step_heads: BTreeMap<String, u32>,
    pub if_total: u32,
    pub if_parsed: u32,
    pub assert_total: u32,
    pub assert_parsed: u32,
    pub foreach_total: u32,
    pub foreach_bound: u32,
    /// `"let:path"`, `"return:call"`, `"return:none"`, … → count.
    pub expr_forms: BTreeMap<String, u32>,
    pub calls_by_form: BTreeMap<String, u32>,
    pub undeclared_vars: u32,
    pub undeclared_review: Vec<ReviewItem>,
    pub assert_review: Vec<ReviewItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub subject: String,
    pub step_path: Option<String>,
    pub target: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateSpec {
    pub representation_version: String,
    pub spec: String,
    pub snapshot_sha: String,
    pub model: ObjectModel,
    pub sources: Vec<crate::state::ir::StatementSource>,
    pub statements: Vec<crate::state::ir::Statement>,
    pub occurrences: Vec<Occurrence>,
    #[serde(default)]
    pub prose_mentions: BTreeMap<String, u32>,
    pub coverage: CoverageCounters,
    pub issues: Vec<ModelIssue>,
    /// `Declared` sites of `state.write`/`state.init` rules that matched no link.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared_sites: Vec<Site>,
    #[serde(default)]
    pub signatures: Vec<Signature>,
    #[serde(default)]
    pub calls: Vec<crate::state::ir::Call>,
    #[serde(default)]
    pub var_origins: Vec<crate::state::ir::VarOrigins>,
    #[serde(default)]
    pub link_roles: Vec<crate::state::ir::SourceLinkRoles>,
    #[serde(default)]
    pub summaries: Vec<AlgorithmSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_key_round_trips_through_its_string_form() {
        let idl = TypeKey::Idl("Document".into());
        let concept = TypeKey::Anchor(AnchorTarget {
            spec: "HTML".into(),
            anchor: "navigable".into(),
        });
        assert_eq!(idl.to_string(), "idl:Document");
        assert_eq!(concept.to_string(), "HTML#navigable");
        assert_eq!(TypeKey::parse("idl:Document"), Some(idl.clone()));
        assert_eq!(TypeKey::parse("HTML#navigable"), Some(concept.clone()));
        assert_eq!(TypeKey::parse("nonsense"), None);
        assert_eq!(serde_json::to_string(&idl).unwrap(), "\"idl:Document\"");
        assert_eq!(
            serde_json::from_str::<TypeKey>("\"HTML#navigable\"").unwrap(),
            concept
        );
    }

    #[test]
    fn type_expr_serializes_with_snake_case_tags() {
        let ty = TypeExpr::Union(vec![
            TypeExpr::Nominal {
                ty: TypeRef::Known(TypeKey::parse("HTML#navigable").unwrap()),
                text: "navigable".into(),
            },
            TypeExpr::Null,
        ]);
        let json = serde_json::to_value(&ty).unwrap();
        assert_eq!(json["union"][1], "null");
        assert_eq!(
            serde_json::to_value(TypeExpr::Primitive(Primitive::Boolean)).unwrap()["primitive"],
            "boolean"
        );
    }

    #[test]
    fn signature_forms_have_stable_tags() {
        let forms = [
            (SignatureForm::To, "to"),
            (SignatureForm::WhenStepsSay, "when_steps_say"),
            (SignatureForm::GivenList, "given_list"),
            (
                SignatureForm::IdlMethod {
                    interface: "Event".into(),
                    member: "initEvent".into(),
                },
                "idl_method",
            ),
            (
                SignatureForm::IdlGetter {
                    interface: "A".into(),
                    member: "b".into(),
                },
                "idl_getter",
            ),
            (
                SignatureForm::IdlSetter {
                    interface: "A".into(),
                    member: "b".into(),
                },
                "idl_setter",
            ),
            (
                SignatureForm::IdlConstructor {
                    interface: "A".into(),
                },
                "idl_constructor",
            ),
            (SignatureForm::Accessor, "accessor"),
            (SignatureForm::Predicate, "predicate"),
            (SignatureForm::Declared, "declared"),
            (SignatureForm::Ecmarkup, "ecmarkup"),
        ];
        for (form, tag) in forms {
            assert_eq!(form.as_str(), tag);
        }
    }

    #[test]
    fn template_and_type_serialization_shapes() {
        let pieces = vec![
            TemplatePiece::Callee,
            TemplatePiece::Slot(0),
            TemplatePiece::Literal("to".into()),
            TemplatePiece::NamedGroup("with".into()),
        ];
        assert_eq!(
            serde_json::to_value(&pieces).unwrap(),
            serde_json::json!(["callee", {"slot": 0}, {"literal": "to"}, {"named_group": "with"}])
        );
        assert_eq!(
            serde_json::to_value(TypeExpr::Idl {
                text: "DOMString".into()
            })
            .unwrap(),
            serde_json::json!({"idl": {"text": "DOMString"}})
        );
        assert_eq!(
            serde_json::to_value(Literal::Failure).unwrap(),
            serde_json::json!("failure")
        );
        assert_eq!(
            serde_json::to_value(Passing::Positional { index: 2 }).unwrap(),
            serde_json::json!({"positional": {"index": 2}})
        );
    }

    #[test]
    fn stored_payload_without_the_new_fields_still_deserializes() {
        let spec = StateSpec {
            representation_version: STATE_VERSION.into(),
            spec: "HTML".into(),
            snapshot_sha: "hash:x".into(),
            model: ObjectModel {
                types: vec![],
                fields: vec![],
                members: vec![],
                reflections: vec![],
            },
            sources: vec![],
            statements: vec![],
            occurrences: vec![],
            prose_mentions: Default::default(),
            coverage: CoverageCounters::default(),
            issues: vec![],
            ..Default::default()
        };
        let mut json = serde_json::to_value(&spec).unwrap();
        for key in [
            "signatures",
            "calls",
            "var_origins",
            "link_roles",
            "summaries",
        ] {
            json.as_object_mut().unwrap().remove(key);
        }
        let back: StateSpec = serde_json::from_value(json).unwrap();
        assert!(back.signatures.is_empty() && back.calls.is_empty() && back.summaries.is_empty());
    }
}
