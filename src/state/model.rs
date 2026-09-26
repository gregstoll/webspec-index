use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

pub use crate::parse::steps::{AnchorTarget, InlineToken, InlineTokenKind, LinkSpan, TextSpan};
pub use crate::state::catalog::StateCatalog;

pub const STATE_VERSION: &str = "4";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectModel {
    pub types: Vec<TypeDef>,
    pub fields: Vec<FieldDef>,
    pub members: Vec<SetMember>,
    pub reflections: Vec<Reflection>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub subject: String,
    pub step_path: Option<String>,
    pub target: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}
