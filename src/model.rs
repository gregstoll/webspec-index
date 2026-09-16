use serde::{Deserialize, Serialize};

/// Options for PR-aware queries.
#[derive(Debug, Clone)]
pub struct PrOpts {
    pub pr_number: i64,
    pub force_update: bool,
}

/// Type of a section
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionType {
    Heading,
    Algorithm,
    Definition,
    Idl,
    Prose,
}

impl SectionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SectionType::Heading => "heading",
            SectionType::Algorithm => "algorithm",
            SectionType::Definition => "definition",
            SectionType::Idl => "idl",
            SectionType::Prose => "prose",
        }
    }
}

impl std::str::FromStr for SectionType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "heading" => Ok(SectionType::Heading),
            "algorithm" => Ok(SectionType::Algorithm),
            "definition" => Ok(SectionType::Definition),
            "idl" => Ok(SectionType::Idl),
            "prose" => Ok(SectionType::Prose),
            _ => Err(()),
        }
    }
}

/// A parsed section from the spec HTML
#[derive(Debug, Clone)]
pub struct ParsedSection {
    pub anchor: String,
    pub title: Option<String>,
    pub content_text: Option<String>,
    pub section_type: SectionType,
    pub parent_anchor: Option<String>,
    pub prev_anchor: Option<String>,
    pub next_anchor: Option<String>,
    pub depth: Option<u8>, // 2-6 for headings
    /// Section number extracted from span.secno / span.secnum, e.g. "7.4.2".
    /// Only set for heading and algorithm (emu-clause) sections; None otherwise.
    pub number: Option<String>,
}

/// Where in a section a cross-reference occurs. Only `Step` references are
/// algorithm invocations; the rest are mentions and should not be treated as
/// call edges when tracing control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefKind {
    /// Inside a numbered algorithm step.
    Step,
    /// Inside a note or example callout, including one nested in a step.
    Note,
    /// Inside a WebIDL block.
    Idl,
    /// Ordinary prose.
    Prose,
}

impl RefKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RefKind::Step => "step",
            RefKind::Note => "note",
            RefKind::Idl => "idl",
            RefKind::Prose => "prose",
        }
    }
}

impl std::str::FromStr for RefKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "step" => Ok(RefKind::Step),
            "note" => Ok(RefKind::Note),
            "idl" => Ok(RefKind::Idl),
            "prose" => Ok(RefKind::Prose),
            _ => Err(()),
        }
    }
}

/// A cross-reference found in the spec
#[derive(Debug, Clone)]
pub struct ParsedReference {
    pub from_anchor: String,
    pub to_spec: String, // Target spec name (same as source for intra-spec refs)
    pub to_anchor: String,
    /// Dotted step number of the enclosing algorithm step, e.g. "24.1".
    pub step_path: Option<String>,
    /// Text of the enclosing step, excluding its substeps.
    pub step_text: Option<String>,
    /// Text of the enclosing steps, outermost first. Empty for top-level steps.
    pub guard_path: Vec<String>,
    /// The `id` the spec generator put on the link element, if any. Lets callers
    /// link to the call site rather than the target's definition.
    pub call_site_id: Option<String>,
    pub kind: RefKind,
}

impl ParsedReference {
    /// A reference with no step context, as produced by prose links.
    pub fn prose(from_anchor: &str, to_spec: &str, to_anchor: &str) -> Self {
        ParsedReference {
            from_anchor: from_anchor.to_string(),
            to_spec: to_spec.to_string(),
            to_anchor: to_anchor.to_string(),
            step_path: None,
            step_text: None,
            guard_path: Vec::new(),
            call_site_id: None,
            kind: RefKind::Prose,
        }
    }
}

/// A parsed WebIDL definition from `dfn[data-dfn-type]`
#[derive(Debug, Clone)]
pub struct ParsedIdlDefinition {
    pub anchor: String,
    pub name: String,
    pub owner: Option<String>,
    pub kind: String,
    pub canonical_name: String,
    pub idl_text: Option<String>,
}

/// Complete parsed spec
#[derive(Debug)]
pub struct ParsedSpec {
    pub sections: Vec<ParsedSection>,
    pub references: Vec<ParsedReference>,
    pub idl_definitions: Vec<ParsedIdlDefinition>,
}

/// JSON output for query command
#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub spec: String,
    pub sha: String,
    pub anchor: String,
    pub url: String,
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    #[serde(rename = "type")]
    pub section_type: String,
    pub content: Option<String>,
    pub navigation: Navigation,
    pub outgoing_refs: Vec<RefEntry>,
    pub incoming_refs: Vec<RefEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Navigation {
    pub parent: Option<NavEntry>,
    pub prev: Option<NavEntry>,
    pub next: Option<NavEntry>,
    pub children: Vec<NavEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavEntry {
    pub anchor: String,
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefEntry {
    pub spec: String,
    pub anchor: String,
    /// Step of the referencing section where the link occurs, e.g. "24.1".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_text: Option<String>,
    /// Enclosing steps, outermost first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub guard_path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_site_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl RefEntry {
    /// A reference with no call-site detail, for callers that only need identity.
    pub fn plain(spec: impl Into<String>, anchor: impl Into<String>) -> Self {
        RefEntry {
            spec: spec.into(),
            anchor: anchor.into(),
            step_path: None,
            step_text: None,
            guard_path: Vec::new(),
            call_site_id: None,
            kind: None,
        }
    }
}

/// JSON output for exists command
#[derive(Debug, Serialize)]
pub struct ExistsResult {
    pub exists: bool,
    pub spec: String,
    pub anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub section_type: Option<String>,
}

/// JSON output for anchors command
#[derive(Debug, Serialize)]
pub struct AnchorsResult {
    pub pattern: String,
    pub results: Vec<AnchorEntry>,
}

#[derive(Debug, Serialize)]
pub struct AnchorEntry {
    pub spec: String,
    pub anchor: String,
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub section_type: String,
}

/// JSON output for search command
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub query: String,
    pub results: Vec<SearchEntry>,
}

#[derive(Debug, Serialize)]
pub struct SearchEntry {
    pub spec: String,
    pub anchor: String,
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub section_type: String,
    pub snippet: String,
}

/// JSON output for list command
#[derive(Debug, Serialize)]
pub struct ListEntry {
    pub anchor: String,
    pub title: Option<String>,
    pub depth: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
}

/// JSON output for spec_urls command
#[derive(Debug, Serialize)]
pub struct SpecUrlEntry {
    pub spec: String,
    pub base_url: String,
}

/// JSON output for update command
#[derive(Debug, Serialize)]
pub struct UpdateEntry {
    pub spec: String,
    pub updated: bool,
}

/// JSON output for graph command
#[derive(Debug, Serialize)]
pub struct GraphResult {
    pub root: GraphRoot,
    pub direction: String,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub truncated: bool,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Serialize)]
pub struct GraphRoot {
    pub spec: String,
    pub anchor: String,
}

#[derive(Debug, Serialize)]
pub struct GraphNode {
    pub id: String, // "{spec}#{anchor}"
    pub spec: String,
    pub anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub section_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_role: Option<String>, // root | matched | bridge
}

#[derive(Debug, Serialize)]
pub struct GraphEdge {
    pub from: String, // node id
    pub to: String,   // node id
    pub kind: String, // currently always "reference"
}

/// JSON output for refs command
#[derive(Debug, Serialize)]
pub struct RefsResult {
    pub query: String,
    pub direction: String,
    pub matches: Vec<RefsMatch>,
}

#[derive(Debug, Serialize)]
pub struct RefsMatch {
    pub spec: String,
    pub anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub section_type: String,
    pub resolution: String, // exact | heuristic
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outgoing: Option<Vec<RefEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incoming: Option<Vec<RefEntry>>,
}

/// One call in a traced path: the step of `spec#anchor` that invokes
/// `to_spec#to_anchor`.
#[derive(Debug, Clone, Serialize)]
pub struct TraceHop {
    pub spec: String,
    pub anchor: String,
    pub to_spec: String,
    pub to_anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub guard_path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_site_id: Option<String>,
    /// Deep link to the call site itself rather than the callee's definition,
    /// when the spec generator emitted a per-reference id for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_site_url: Option<String>,
    /// Canonical URL of the calling section itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Trace {
    pub hops: Vec<TraceHop>,
}

/// How much of each hop a trace should carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceDetail {
    /// Every hop with its step text, guards and call-site link.
    Verbose,
    /// `SPEC#anchor` plus step number, no prose and no URLs. Smallest form, and
    /// every token is an identifier the other commands accept.
    Edges,
    /// One line per hop with links, collapsing an edge's call sites onto that
    /// line rather than repeating the route once per call site.
    Compact,
}

impl TraceDetail {
    pub fn as_str(&self) -> &'static str {
        match self {
            TraceDetail::Verbose => "verbose",
            TraceDetail::Edges => "edges",
            TraceDetail::Compact => "compact",
        }
    }
}

impl std::str::FromStr for TraceDetail {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "verbose" => Ok(TraceDetail::Verbose),
            "edges" => Ok(TraceDetail::Edges),
            "compact" => Ok(TraceDetail::Compact),
            _ => Err(()),
        }
    }
}

/// JSON output for trace command
#[derive(Debug, Serialize)]
pub struct TraceResult {
    pub from: String,
    pub to: String,
    pub max_depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub traces: Vec<Trace>,
    /// True when the search hit its trace or node budget, so absence of a route
    /// is not proof that none exists.
    pub truncated: bool,
}

impl TraceResult {
    /// Drop whatever the requested detail level does not carry.
    ///
    /// `Edges` leaves `SPEC#anchor` plus a step number — the tool's own
    /// identifier format, so a hop can be fed straight back into `query` or
    /// `refs`, which a call-site URL cannot since it names a link rather than a
    /// section. `call_site_id` survives in JSON for callers rebuilding the URL.
    ///
    /// Neither reduced level is a cheaper way to judge a route: the guards they
    /// drop are exactly what settles whether a route is taken, and reading
    /// sections to recover them costs far more than carrying them did.
    pub fn apply_detail(&mut self, detail: TraceDetail) {
        if detail == TraceDetail::Verbose {
            return;
        }
        for trace in &mut self.traces {
            for hop in &mut trace.hops {
                hop.step_text = None;
                hop.guard_path.clear();
                if detail == TraceDetail::Edges {
                    hop.call_site_url = None;
                    hop.url = None;
                }
            }
        }
    }
}

/// JSON output for idl command
#[derive(Debug, Serialize)]
pub struct IdlResult {
    pub query: String,
    pub matches: Vec<IdlEntry>,
}

/// JSON output for pr-diff command
#[derive(Debug, Serialize)]
pub struct PrDiffResult {
    pub spec: String,
    pub pr_number: i64,
    pub head_sha: String,
    pub merge_base_sha: String,
    pub summary: PrDiffSummary,
    pub changes: Vec<PrDiffEntry>,
}

#[derive(Debug, Serialize)]
pub struct PrDiffSummary {
    pub added: usize,
    pub removed: usize,
    pub modified: usize,
}

#[derive(Debug, Serialize)]
pub struct PrDiffEntry {
    pub anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub change_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_content: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IdlEntry {
    pub spec: String,
    pub anchor: String,
    pub kind: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub canonical_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idl_text: Option<String>,
}

// ─── Flow extraction types ────────────────────────────────────────────────────

/// Control-flow graph of one algorithm section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowResult {
    pub spec: String,
    pub anchor: String,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
    pub issues: Vec<FlowIssue>,
}

/// One step (or synthesised node) in a control-flow graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowNode {
    /// Dotted step path, e.g. `"5.2"`, or `"DOM#concept-tree"` for external call targets.
    pub id: String,
    pub kind: FlowNodeKind,
    /// Plain text of the step, at most 120 chars (truncated with `…`).
    pub text: String,
    /// Algorithm invocations made from this step.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub calls: Vec<FlowCall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowNodeKind {
    Step,
    Branch,
    Loop,
    Parallel,
    Terminal,
    External,
}

/// A cross-section algorithm invocation attached to a step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowCall {
    pub spec: String,
    pub anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub kind: FlowEdgeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowEdgeKind {
    Next,
    Then,
    Else,
    Loop,
    Jump,
    Unknown,
    Call,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowIssue {
    pub step: String,
    pub code: String,
    pub message: String,
}
