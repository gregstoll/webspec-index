use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const EFFECTS_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_ENVIRONMENT: &str = "web";
pub const DEFAULT_MAX_BODIES: u64 = 20_000;
pub const DEFAULT_MAX_RELATIONSHIPS: u64 = 500_000;
pub const DEFAULT_MAX_STATES: u64 = 2_000_000;
pub const DEFAULT_MAX_WITNESS_STATES: u64 = 20_000;
// Reverse BFS visits each state once. Its work budget already bounds path
// length; don't impose an additional shallow cutoff on default explanations.
pub const DEFAULT_MAX_DEPTH: u64 = DEFAULT_MAX_WITNESS_STATES;
pub const DEFAULT_WITNESS_LIMIT: u64 = 1;

/// A catalog value. `None` is serialized as an explicit unknown (`null`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EffectValue {
    String(String),
    Boolean(bool),
    Integer(i64),
}

pub type EffectParams = BTreeMap<String, Option<EffectValue>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectSelector {
    pub spec: String,
    pub anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_path: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_id: Option<String>,
}

impl SubjectSelector {
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.spec.is_empty() || self.anchor.is_empty() {
            return Err(RequestError::invalid(
                "subject spec and anchor must be non-empty",
            ));
        }
        let selectors = usize::from(self.step_path.is_some())
            + usize::from(self.step_id.is_some())
            + usize::from(self.body_id.is_some());
        if selectors > 1 {
            return Err(RequestError::invalid(
                "subject may contain at most one of step_path, step_id, or body_id",
            ));
        }
        if self.step_path.as_ref().is_some_and(Vec::is_empty) {
            return Err(RequestError::invalid("step_path must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub spec: String,
    pub anchor: String,
    pub snapshot_sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_path: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Execution {
    Inline,
    Separate,
    Unknown,
}

impl Execution {
    /// Compose execution along one route. A separate boundary remains separate;
    /// otherwise unresolved timing dominates inline execution.
    pub fn compose(self, other: Self) -> Self {
        match (self, other) {
            (Self::Separate, _) | (_, Self::Separate) => Self::Separate,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::Inline,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectSummary {
    pub id: String,
    pub kind: String,
    pub params: EffectParams,
    /// Must contain sorted, distinct values. Use [`sorted_execution`] at boundaries.
    pub execution: Vec<Execution>,
    /// One concrete detection site, preferring numbered steps over declarations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<EffectLocation>,
    /// A bounded sample of further sites; included in `additional_locations`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_locations: Vec<EffectLocation>,
    /// Other distinct source locations represented by this group.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub additional_locations: u64,
}

pub const MAX_SUMMARY_LOCATIONS: usize = 3;

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectLocation {
    pub spec: String,
    pub anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_path: Option<Vec<u32>>,
    pub url: String,
}

pub fn sorted_execution(values: impl IntoIterator<Item = Execution>) -> Vec<Execution> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    /// Inclusive UTF-8 byte offset.
    pub start: u64,
    /// Exclusive UTF-8 byte offset.
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSite {
    pub id: String,
    pub subject: Subject,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceBasis {
    Matched,
    Declared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub id: String,
    pub basis: EvidenceBasis,
    pub rule_id: String,
    pub site: SourceSite,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captures: Option<BTreeMap<String, Option<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_expressions: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    Invoke,
    CandidateInvoke,
    DefineBody,
    ScheduleBody,
    Resume,
    Implements,
    Mention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Enclosing,
    Branch,
    Binding,
    PrecedingExit,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextItem {
    pub kind: ContextKind,
    pub text: String,
    pub site: SourceSite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryEffect {
    pub kind: String,
    pub params: EffectParams,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    pub execution: Execution,
    pub operation_site_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<BoundaryEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessHop {
    pub from: Subject,
    pub to: Subject,
    pub relation: Relationship,
    pub site: SourceSite,
    pub context: Vec<ContextItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<Boundary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathFeasibility {
    Unchecked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCode {
    MissingSpec,
    MissingAnchor,
    UnsupportedStructure,
    UnresolvedInvocation,
    UnresolvedBodyBinding,
    UnresolvedArgument,
    AmbiguousMatch,
    DeclarationMismatch,
    AnalysisBudget,
    SnapshotChanged,
    UnsupportedPreview,
    WitnessBudget,
    ContextTruncated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Issue {
    pub code: IssueCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<SourceSite>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Witness {
    pub hops: Vec<WitnessHop>,
    pub terminal_evidence: Vec<Evidence>,
    pub path_feasibility: PathFeasibility,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectExplanation {
    pub effect_id: String,
    pub witnesses: Vec<Witness>,
    pub witnesses_truncated: bool,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Semantics {
    #[default]
    May,
}

/// The shape makes illegal status combinations unrepresentable. Only `ready`
/// can carry a coverage level and analysis identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectsStatus {
    Ready {
        semantics: Semantics,
        coverage: Coverage,
        issues: Vec<IssueCode>,
        omitted: u64,
        analysis_id: String,
    },
    Pending {
        semantics: Semantics,
        issues: Vec<IssueCode>,
        omitted: u64,
    },
    Unavailable {
        semantics: Semantics,
        issues: Vec<IssueCode>,
        omitted: u64,
    },
    Error {
        semantics: Semantics,
        issues: Vec<IssueCode>,
        omitted: u64,
    },
    Disabled {
        semantics: Semantics,
        issues: Vec<IssueCode>,
        omitted: u64,
    },
}

impl EffectsStatus {
    pub fn ready(
        coverage: Coverage,
        issues: Vec<IssueCode>,
        omitted: u64,
        analysis_id: String,
    ) -> Self {
        Self::Ready {
            semantics: Semantics::May,
            coverage,
            issues,
            omitted,
            analysis_id,
        }
    }

    /// Validate semantic constraints not expressible by serde's tagged enum.
    pub fn validate(&self) -> Result<(), RequestError> {
        match self {
            Self::Pending { omitted, .. }
            | Self::Unavailable { omitted, .. }
            | Self::Error { omitted, .. }
            | Self::Disabled { omitted, .. }
                if *omitted != 0 =>
            {
                Err(RequestError::invalid(
                    "a non-ready effects status must have omitted equal to 0",
                ))
            }
            _ => Ok(()),
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinedBody {
    pub subject: Subject,
    pub effects: Vec<EffectSummary>,
    pub effects_status: EffectsStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSpec {
    pub spec: String,
    pub snapshot_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissingInput {
    pub spec: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryBudgets {
    pub max_bodies: u64,
    pub max_relationships: u64,
    pub max_states: u64,
}

impl Default for DiscoveryBudgets {
    fn default() -> Self {
        Self {
            max_bodies: DEFAULT_MAX_BODIES,
            max_relationships: DEFAULT_MAX_RELATIONSHIPS,
            max_states: DEFAULT_MAX_STATES,
        }
    }
}

impl DiscoveryBudgets {
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.max_bodies == 0 || self.max_relationships == 0 || self.max_states == 0 {
            return Err(RequestError::invalid("discovery budgets must be positive"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputManifest {
    pub specs: Vec<ManifestSpec>,
    pub structural_representation_version: u32,
    pub registry_resolution_version: u32,
    pub environment: String,
    pub catalog_digest: String,
    pub analysis_engine_version: u32,
    pub scope: AnalysisScope,
    pub budget_profile: DiscoveryBudgets,
    pub missing_inputs: Vec<MissingInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalysisScope {
    All,
    Subject { subject: SubjectSelector },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectSummaryResult {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u32,
    pub subject: Subject,
    pub effects: Vec<EffectSummary>,
    pub effects_status: EffectsStatus,
    pub defined_bodies: Vec<DefinedBody>,
    pub issues: Vec<Issue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_manifest: Option<InputManifest>,
}

impl EffectSummaryResult {
    pub fn validate(&self) -> Result<(), RequestError> {
        validate_result_contract(
            self.schema_version,
            &self.effects,
            &self.effects_status,
            &self.defined_bodies,
            self.input_manifest.as_ref(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplainEffectsResult {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u32,
    pub subject: Subject,
    pub effects: Vec<EffectSummary>,
    pub effects_status: EffectsStatus,
    pub defined_bodies: Vec<DefinedBody>,
    pub explanations: Vec<EffectExplanation>,
    pub issues: Vec<Issue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_manifest: Option<InputManifest>,
}

impl ExplainEffectsResult {
    pub fn validate(&self) -> Result<(), RequestError> {
        validate_result_contract(
            self.schema_version,
            &self.effects,
            &self.effects_status,
            &self.defined_bodies,
            self.input_manifest.as_ref(),
        )?;
        let effect_ids: BTreeSet<_> = self.effects.iter().map(|effect| &effect.id).collect();
        let explanation_ids: BTreeSet<_> = self
            .explanations
            .iter()
            .map(|explanation| &explanation.effect_id)
            .collect();
        if effect_ids.len() != self.effects.len()
            || explanation_ids.len() != self.explanations.len()
            || effect_ids != explanation_ids
        {
            return Err(RequestError::invalid(
                "explanations must contain exactly one entry for every selected effect",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EffectsMode {
    #[default]
    Auto,
    Cached,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EffectsOptions {
    pub mode: EffectsMode,
    pub rule_paths: Vec<String>,
    pub environment: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_id: Option<String>,
    pub budgets: DiscoveryBudgets,
}

impl Default for EffectsOptions {
    fn default() -> Self {
        Self {
            mode: EffectsMode::Auto,
            rule_paths: Vec::new(),
            environment: DEFAULT_ENVIRONMENT.to_string(),
            analysis_id: None,
            budgets: DiscoveryBudgets::default(),
        }
    }
}

impl EffectsOptions {
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.environment.is_empty() {
            return Err(RequestError::invalid("environment must be non-empty"));
        }
        self.budgets.validate()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EffectFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectsRequest {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u32,
    pub subject: SubjectSelector,
    #[serde(default)]
    pub options: EffectsOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<EffectFilter>,
}

impl EffectsRequest {
    pub fn validate(&self) -> Result<(), RequestError> {
        validate_version(self.schema_version)?;
        self.subject.validate()?;
        self.options.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExplanationOptions {
    pub max_depth: u64,
    pub max_states: u64,
    pub limit: u64,
}

impl Default for ExplanationOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_states: DEFAULT_MAX_WITNESS_STATES,
            limit: DEFAULT_WITNESS_LIMIT,
        }
    }
}

impl ExplanationOptions {
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.max_depth == 0 || self.max_states == 0 || self.limit == 0 {
            return Err(RequestError::invalid("explanation limits must be positive"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplainEffectsRequest {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u32,
    pub subject: SubjectSelector,
    #[serde(default)]
    pub options: EffectsOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<EffectFilter>,
    #[serde(default)]
    pub explanation: ExplanationOptions,
}

impl ExplainEffectsRequest {
    pub fn validate(&self) -> Result<(), RequestError> {
        validate_version(self.schema_version)?;
        self.subject.validate()?;
        self.options.validate()?;
        self.explanation.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecomputeEffectsRequest {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u32,
    pub scope: AnalysisScope,
    #[serde(default)]
    pub options: EffectsOptions,
}

impl RecomputeEffectsRequest {
    pub fn validate(&self) -> Result<(), RequestError> {
        validate_version(self.schema_version)?;
        if self.options.analysis_id.is_some() {
            return Err(RequestError::invalid(
                "recomputation is incompatible with a historical analysis_id",
            ));
        }
        if self.options.mode != EffectsMode::Auto {
            return Err(RequestError::invalid("recomputation requires mode auto"));
        }
        if let AnalysisScope::Subject { subject } = &self.scope {
            subject.validate()?;
        }
        self.options.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecomputeEffectsResult {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u32,
    pub analysis_id: String,
    pub input_manifest: InputManifest,
    pub processed_subjects: Vec<Subject>,
    pub unprocessed_subjects: Vec<Subject>,
    pub body_count: u64,
    pub relationship_count: u64,
    pub state_count: u64,
    pub issues: Vec<Issue>,
}

impl RecomputeEffectsResult {
    pub fn validate(&self) -> Result<(), RequestError> {
        validate_version(self.schema_version)?;
        if self.analysis_id.is_empty() {
            return Err(RequestError::invalid("analysis_id must be non-empty"));
        }
        self.input_manifest.budget_profile.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestErrorCode {
    InvalidRequest,
    InvalidCatalog,
    SubjectNotFound,
    AmbiguousSubject,
    EffectNotFound,
    OccurrenceNotFound,
    AnalysisUnavailable,
    AnalysisFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestError {
    pub code: RequestErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl RequestError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: RequestErrorCode::InvalidRequest,
            message: message.into(),
            details: None,
        }
    }
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RequestError {}

fn validate_version(version: u32) -> Result<(), RequestError> {
    if version == EFFECTS_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(RequestError::invalid(format!(
            "unsupported effects schema version {version}; expected {EFFECTS_SCHEMA_VERSION}"
        )))
    }
}

fn validate_result_contract(
    version: u32,
    effects: &[EffectSummary],
    status: &EffectsStatus,
    defined_bodies: &[DefinedBody],
    manifest: Option<&InputManifest>,
) -> Result<(), RequestError> {
    validate_version(version)?;
    status.validate()?;
    for effect in effects {
        validate_effect_summary(effect)?;
    }
    for body in defined_bodies {
        body.effects_status.validate()?;
        for effect in &body.effects {
            validate_effect_summary(effect)?;
        }
        if !body.effects_status.is_ready() && !body.effects.is_empty() {
            return Err(RequestError::invalid(
                "a non-ready defined body status cannot carry effects",
            ));
        }
    }
    if status.is_ready() {
        let manifest = manifest.ok_or_else(|| {
            RequestError::invalid("a ready effects result requires an input_manifest")
        })?;
        manifest.budget_profile.validate()?;
    } else if !effects.is_empty() || manifest.is_some() {
        return Err(RequestError::invalid(
            "a non-ready effects result must have empty effects and omit input_manifest",
        ));
    }
    Ok(())
}

fn validate_effect_summary(effect: &EffectSummary) -> Result<(), RequestError> {
    if effect.id.is_empty() || effect.kind.is_empty() || effect.execution.is_empty() {
        return Err(RequestError::invalid(
            "effect id, kind, and execution must be non-empty",
        ));
    }
    if sorted_execution(effect.execution.iter().copied()) != effect.execution {
        return Err(RequestError::invalid(
            "effect execution values must be sorted and distinct",
        ));
    }
    Ok(())
}

fn deserialize_schema_version<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version == EFFECTS_SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(format!(
            "unsupported effects schema version {version}; expected {EFFECTS_SCHEMA_VERSION}"
        )))
    }
}

/// Encode JSON canonically: recursively sorted object keys, compact UTF-8, and
/// array order preserved. This is the digest surface for v1 contracts.
pub fn canonical_json(value: &Value) -> Result<String, serde_json::Error> {
    fn sort(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by_key(|(key, _)| *key);
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.clone(), sort(value)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(sort).collect()),
            _ => value.clone(),
        }
    }
    serde_json::to_string(&sort(value))
}

pub fn canonical_json_sha256(value: &Value) -> Result<String, serde_json::Error> {
    let canonical = canonical_json(value)?;
    Ok(hex_sha256(canonical.as_bytes()))
}

pub fn digest_serializable(value: &impl Serialize) -> Result<String, serde_json::Error> {
    canonical_json_sha256(&serde_json::to_value(value)?)
}

pub fn effect_digest(kind: &str, params: &EffectParams) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct Identity<'a> {
        kind: &'a str,
        params: &'a EffectParams,
    }
    digest_serializable(&Identity { kind, params })
}

/// Produce collision-safe effect handles for a run. Equal full digests share a
/// handle; distinct digests extend together beyond the normal 16 hex digits.
pub fn effect_handles(full_digests: &[String]) -> Vec<String> {
    let mut lengths = vec![16usize; full_digests.len()];
    loop {
        let mut changed = false;
        for left in 0..full_digests.len() {
            for right in (left + 1)..full_digests.len() {
                if full_digests[left] == full_digests[right] {
                    continue;
                }
                let left_len = lengths[left].min(full_digests[left].len());
                let right_len = lengths[right].min(full_digests[right].len());
                if full_digests[left][..left_len] == full_digests[right][..right_len] {
                    let next = left_len.max(right_len) + 1;
                    lengths[left] = next.min(full_digests[left].len());
                    lengths[right] = next.min(full_digests[right].len());
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    full_digests
        .iter()
        .zip(lengths)
        .map(|(digest, len)| format!("ef_{}", &digest[..len.min(digest.len())]))
        .collect()
}

pub fn analysis_id(manifest: &InputManifest, generation: i64) -> Result<String, serde_json::Error> {
    Ok(format!(
        "an_{}",
        &digest_serializable(&(manifest, generation))?[..16]
    ))
}

pub fn source_node_id(snapshot_sha: &str, structural_location: &str) -> String {
    let digest = hex_sha256(format!("{snapshot_sha}\0{structural_location}").as_bytes());
    format!("src_{}", &digest[..24])
}

pub fn canonical_anchor_identity(spec: &str, anchor: &str) -> String {
    format!("{}#{}", spec.trim(), anchor.trim().trim_start_matches('#'))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_objects_but_preserves_arrays() {
        let value = json!({"z": 1, "a": {"y": 2, "x": [3, 1]}});
        assert_eq!(
            canonical_json(&value).unwrap(),
            r#"{"a":{"x":[3,1],"y":2},"z":1}"#
        );
    }

    #[test]
    fn execution_composition_and_sorting_match_contract() {
        assert_eq!(
            Execution::Inline.compose(Execution::Unknown),
            Execution::Unknown
        );
        assert_eq!(
            Execution::Unknown.compose(Execution::Separate),
            Execution::Separate
        );
        assert_eq!(
            sorted_execution([Execution::Unknown, Execution::Inline, Execution::Inline]),
            vec![Execution::Inline, Execution::Unknown]
        );
    }

    #[test]
    fn collision_safe_handles_extend_both_digests() {
        let digests = vec![
            "0123456789abcdef0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            "0123456789abcdef0bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            "fedcba9876543210cccccccccccccccccccccccccccccccccccccccccccccccc".into(),
        ];
        let handles = effect_handles(&digests);
        assert_eq!(handles[0], "ef_0123456789abcdef0a");
        assert_eq!(handles[1], "ef_0123456789abcdef0b");
        assert_eq!(handles[2], "ef_fedcba9876543210");
    }

    #[test]
    fn selector_rejects_multiple_precise_selectors() {
        let selector = SubjectSelector {
            spec: "TEST".into(),
            anchor: "r".into(),
            step_path: Some(vec![1]),
            step_id: Some("step-1".into()),
            body_id: None,
        };
        assert_eq!(
            selector.validate().unwrap_err().code,
            RequestErrorCode::InvalidRequest
        );
    }
}
