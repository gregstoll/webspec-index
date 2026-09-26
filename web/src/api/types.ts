// TypeScript types mirroring webspec-index Rust model structs.
// The API uses adjacent tagging: every non-error response is
// { "type": "<variant>", "result": { ...payload } }.
// Errors are flat: { "type": "error", "code": "...", "message": "..." }.
// Field names inside result match the JSON produced by serde.

export type SectionType = 'heading' | 'algorithm' | 'definition' | 'idl' | 'prose';
export type RefKind = 'step' | 'note' | 'idl' | 'prose';

// --- Navigation ---

export interface NavEntry {
  anchor: string;
  title?: string;
  number?: string;
}

// parent/prev/next are serialized as JSON null when absent, not omitted.
export interface Navigation {
  parent: NavEntry | null;
  prev: NavEntry | null;
  next: NavEntry | null;
  children: NavEntry[];
}

// --- References ---

export interface RefEntry {
  spec: string;
  anchor: string;
  step_path?: string;
  step_text?: string;
  guard_path?: string[];
  call_site_id?: string;
  kind?: string;
}

// --- Query ---

export interface QueryResult {
  spec: string;
  sha: string;
  anchor: string;
  url: string;
  title?: string;
  number?: string;
  type: string;          // section type: heading | algorithm | definition | idl | prose
  content?: string;
  content_html?: string; // populated when request includes render: "html"
  navigation: Navigation;
  outgoing_refs: RefEntry[];
  incoming_refs: RefEntry[];
  effects?: EffectSummary[];
  effects_status?: EffectsStatus;
}

// --- Search ---

export interface SearchEntry {
  spec: string;
  anchor: string;
  title?: string;
  type: string;
  snippet: string;
}

export interface SearchResult {
  query: string;
  results: SearchEntry[];
}

// --- Anchors ---

export interface AnchorEntry {
  spec: string;
  anchor: string;
  title?: string;
  type: string;
}

export interface AnchorsResult {
  pattern: string;
  results: AnchorEntry[];
}

// --- List (headings) ---

export interface ListEntry {
  anchor: string;
  title?: string;
  depth: number;
  parent?: string;
  number?: string;
}

// --- Refs ---

export interface RefsMatch {
  spec: string;
  anchor: string;
  title?: string;
  type: string;
  resolution: string;
  outgoing?: RefEntry[];
  incoming?: RefEntry[];
}

export interface RefsResult {
  query: string;
  direction: string;
  matches: RefsMatch[];
}

// --- Trace ---

export interface TraceHop {
  spec: string;
  anchor: string;
  to_spec: string;
  to_anchor: string;
  step_path?: string;
  step_text?: string;
  guard_path?: string[];
  call_site_id?: string;
  call_site_url?: string;
  url?: string;
}

export interface Trace {
  hops: TraceHop[];
}

export interface TraceResult {
  from: string;
  to: string;
  max_depth: number;
  kind?: string;
  traces: Trace[];
  truncated: boolean;
}

// --- Graph ---

export interface GraphRoot {
  spec: string;
  anchor: string;
}

export interface GraphNode {
  id: string;
  spec: string;
  anchor: string;
  title?: string;
  type?: string;
  filter_role?: string;
}

export interface GraphEdge {
  from: string;
  to: string;
  kind: string;
}

export interface GraphResult {
  root: GraphRoot;
  direction: string;
  max_depth: number;
  max_nodes: number;
  truncated: boolean;
  nodes: GraphNode[];
  edges: GraphEdge[];
}

// --- IDL ---

export interface IdlEntry {
  spec: string;
  anchor: string;
  kind: string;
  name: string;
  owner?: string;
  canonical_name: string;
  title?: string;
  idl_text?: string;
}

export interface IdlResult {
  query: string;
  matches: IdlEntry[];
}

// --- Exists ---

export interface ExistsResult {
  exists: boolean;
  spec: string;
  anchor: string;
  type?: string;  // section type when exists is true
}

// --- Specs list ---

export interface SpecEntry {
  name: string;
  base_url: string;
  provider: string;
  sha: string;
  commit_date: string;
}

// --- Effects types ---

export type EffectValue = string | boolean | number;
export type EffectParams = Record<string, EffectValue | null>;

export type Execution = 'inline' | 'separate' | 'unknown';
export type Coverage = 'complete' | 'partial';
export type Semantics = 'may';
export type Relationship = 'invoke' | 'candidate_invoke' | 'define_body' | 'schedule_body' | 'resume' | 'implements' | 'mention';
export type EvidenceBasis = 'matched' | 'declared';
export type ContextKind = 'enclosing' | 'branch' | 'binding' | 'preceding_exit' | 'unsupported';
export type PathFeasibility = 'unchecked';

export type IssueCode =
  | 'missing_spec'
  | 'missing_anchor'
  | 'unsupported_structure'
  | 'unresolved_invocation'
  | 'unresolved_body_binding'
  | 'unresolved_argument'
  | 'ambiguous_match'
  | 'declaration_mismatch'
  | 'analysis_budget'
  | 'snapshot_changed'
  | 'unsupported_preview'
  | 'witness_budget'
  | 'context_truncated';

export interface Subject {
  spec: string;
  anchor: string;
  snapshot_sha: string;
  step_id?: string;
  step_path?: number[];
  body_id?: string;
}

export interface EffectLocation {
  spec: string;
  anchor: string;
  step_path?: number[];
  url: string;
}

export interface EffectSummary {
  id: string;
  kind: string;
  params: EffectParams;
  execution: Execution[];
  location?: EffectLocation;
  other_locations?: EffectLocation[];
  additional_locations?: number;
}

export type EffectsStatus =
  | { state: 'ready'; semantics: Semantics; coverage: Coverage; issues: IssueCode[]; omitted: number; analysis_id: string }
  | { state: 'pending'; semantics: Semantics; issues: IssueCode[]; omitted: number }
  | { state: 'unavailable'; semantics: Semantics; issues: IssueCode[]; omitted: number }
  | { state: 'error'; semantics: Semantics; issues: IssueCode[]; omitted: number }
  | { state: 'disabled'; semantics: Semantics; issues: IssueCode[]; omitted: number };

export interface Span {
  start: number;
  end: number;
}

export interface SourceSite {
  id: string;
  subject: Subject;
  url: string;
  segment_id?: string;
  span?: Span;
  step_text?: string;
}

export interface Issue {
  code: IssueCode;
  message: string;
  site?: SourceSite;
}

export interface Evidence {
  id: string;
  basis: EvidenceBasis;
  rule_id: string;
  site: SourceSite;
  captures?: Record<string, string | null>;
  argument_expressions?: Record<string, string>;
  reason?: string;
}

export interface ContextItem {
  kind: ContextKind;
  text: string;
  site: SourceSite;
}

export interface BoundaryEffect {
  kind: string;
  params: EffectParams;
}

export interface Boundary {
  execution: Execution;
  operation_site_id: string;
  effect?: BoundaryEffect;
}

export interface WitnessHop {
  from: Subject;
  to: Subject;
  relation: Relationship;
  site: SourceSite;
  context: ContextItem[];
  boundary?: Boundary;
}

export interface Witness {
  hops: WitnessHop[];
  terminal_evidence: Evidence[];
  path_feasibility: PathFeasibility;
  issues: Issue[];
}

export interface EffectExplanation {
  effect_id: string;
  witnesses: Witness[];
  witnesses_truncated: boolean;
  issues: Issue[];
}

export interface DefinedBody {
  subject: Subject;
  effects: EffectSummary[];
  effects_status: EffectsStatus;
}

export interface EffectSummaryResult {
  schema_version: number;
  subject: Subject;
  effects: EffectSummary[];
  effects_status: EffectsStatus;
  defined_bodies: DefinedBody[];
  issues: Issue[];
  input_manifest?: unknown;
}

export interface ExplainEffectsResult {
  schema_version: number;
  subject: Subject;
  effects: EffectSummary[];
  effects_status: EffectsStatus;
  defined_bodies: DefinedBody[];
  explanations: EffectExplanation[];
  issues: Issue[];
  input_manifest?: unknown;
}

// --- Effects selectors ---

export interface SubjectSelector {
  spec: string;
  anchor: string;
  step_path?: number[];
  step_id?: string;
  body_id?: string;
}

export interface EffectFilter {
  kind?: string;
  category?: string;
  rule_id?: string;
  effect_id?: string;
  occurrence_id?: string;
}

// --- Flow types ---

export type FlowNodeKind = 'step' | 'branch' | 'loop' | 'parallel' | 'terminal' | 'external';
export type FlowEdgeKind = 'next' | 'then' | 'else' | 'loop' | 'jump' | 'unknown' | 'call';

export interface FlowCall {
  spec: string;
  anchor: string;
  step_path?: string;
}

export interface FlowNode {
  id: string;
  kind: FlowNodeKind;
  text: string;
  calls?: FlowCall[];
}

export interface FlowEdge {
  from: string;
  to: string;
  kind: FlowEdgeKind;
  label?: string;
}

export interface FlowIssue {
  step: string;
  code: string;
  message: string;
}

export interface FlowResult {
  spec: string;
  anchor: string;
  nodes: FlowNode[];
  edges: FlowEdge[];
  issues: FlowIssue[];
}

// --- State types ---

export type StateOccurrenceOp = string;

export interface StateSite { spec: string; subject: string; step_path: string | null; step_id: string | null; context: 'algorithm' | 'branch_label' | 'prose' | 'reflection'; role: string | null; op: StateOccurrenceOp; receiver: string; target: string; value: string | null; text: string; basis: string; constructed: string | null }
export interface StateStatus { state: 'ready'; semantics: Semantics; coverage: Coverage; issues: string[]; counts: { writes: number; inits: number; unclassified: number; possible_unlinked: number; reads: number } }
export interface StateFieldResult { field: { spec: string; anchor: string; name: string; url: string; owners: { type: string; name: string | null; via: string; indexed: boolean }[]; owner_hint: string | null; owner_basis: string | null; field_basis: string | null; declared_type: unknown; initial: unknown | null; declaration: { section: string; text: string } | null }; found_on: { type: string; path: string[] } | null; writes: StateSite[]; inits?: StateSite[]; unclassified: { count: number; items: StateSite[] }; possible_unlinked: StateSite[]; declared: StateSite[]; reflects?: { content_attribute: string | null; name: string; basis: string }; status: StateStatus }
export interface StateFieldRow { spec: string; anchor: string; name: string; declared_type: unknown; initial: unknown | null; basis: string; writes: number; inits: number; unclassified: number }
export interface StateTypeResult { key: string; name: string; kind: string; anchors: { spec: string; anchor: string; role: string }[]; supertypes: string[]; includes: { name: string; spec: string }[]; fields: StateFieldRow[]; dfn_for_only: StateFieldRow[]; members: { spec: string; anchor: string; name: string; adds: number; removes: number }[]; inherited: { from: string; name: string; fields: StateFieldRow[]; dfn_for_only: StateFieldRow[] }[]; truncated: Record<string, number>; status: StateStatus }
export interface StateMemberResult { member: { spec: string; anchor: string; name: string; set: string; set_name: string | null; declaration: { section: string; text: string } }; adds: { count: number; items: StateSite[] }; removes: { count: number; items: StateSite[] }; unclassified: { count: number; items: StateSite[] }; status: StateStatus }
export interface StateFieldListResult { selector: string; total: number; fields: { row: StateFieldRow; owners: StateFieldResult['field']['owners']; found_on: StateFieldResult['found_on']; writes: StateSite[]; inits: StateSite[]; unclassified: StateSite[] }[]; status: StateStatus }
export interface StateCoverageResult { spec: string; snapshot_sha: string; counters: Record<string, unknown> }

// --- Request union ---

export type Request =
  | { type: 'specs' }
  | { type: 'exists'; target: string }
  | { type: 'query'; target: string; effects?: boolean; render?: 'html' }
  | { type: 'search'; query: string; spec?: string; limit?: number }
  | { type: 'anchors'; pattern: string; spec?: string; limit?: number }
  | { type: 'list'; spec: string }
  | { type: 'refs'; target: string; direction?: string; kind?: string; limit?: number }
  | { type: 'trace'; from: string; to: string; max_depth?: number; kind?: string; max_traces?: number }
  | {
      type: 'graph';
      target: string;
      direction?: string;
      max_depth?: number;
      max_nodes?: number;
      include?: string[];
      exclude?: string[];
      same_spec_only?: boolean;
    }
  | { type: 'idl'; query: string; spec?: string; limit?: number }
  | { type: 'effects'; subject: SubjectSelector; filter?: EffectFilter }
  | { type: 'effects_explain'; subject: SubjectSelector }
  | { type: 'effects_paths'; subject: SubjectSelector; effect_id: string; limit?: number }
  | { type: 'flow'; target: string }
  | { type: 'state'; selector: string; include_inits?: boolean; unclassified?: boolean; limit?: number }
  | { type: 'state_coverage'; spec: string };

// --- Error (flat, no result wrapper) ---

export interface ApiError {
  type: 'error';
  code: string;
  message: string;
  details?: unknown;
}

// --- Response union (adjacent tagged: { type, result }) ---

export type Response =
  | ApiError
  | { type: 'specs'; result: { specs: SpecEntry[] } }
  | { type: 'exists'; result: ExistsResult }
  | { type: 'query'; result: QueryResult }
  | { type: 'search'; result: SearchResult }
  | { type: 'anchors'; result: AnchorsResult }
  | { type: 'list'; result: { spec: string; entries: ListEntry[] } }
  | { type: 'refs'; result: RefsResult }
  | { type: 'trace'; result: TraceResult }
  | { type: 'graph'; result: GraphResult }
  | { type: 'idl'; result: IdlResult }
  | { type: 'effects'; result: EffectSummaryResult }
  | { type: 'effects_explain'; result: ExplainEffectsResult }
  | { type: 'effects_paths'; result: ExplainEffectsResult }
  | { type: 'flow'; result: FlowResult }
  | { type: 'state_field'; result: StateFieldResult }
  | { type: 'state_type'; result: StateTypeResult }
  | { type: 'state_member'; result: StateMemberResult }
  | { type: 'state_fields'; result: StateFieldListResult }
  | { type: 'state_coverage'; result: StateCoverageResult };
