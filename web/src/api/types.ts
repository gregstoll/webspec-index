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
  type: string;          // section type: heading | algorithm | definition | idl | prose
  content?: string;
  content_html?: string; // populated when request includes render: "html"
  navigation: Navigation;
  outgoing_refs: RefEntry[];
  incoming_refs: RefEntry[];
  effects?: unknown;
  effects_status?: unknown;
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
  | { type: 'effects_explain'; subject: SubjectSelector };

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
  | { type: 'effects'; result: unknown }
  | { type: 'effects_explain'; result: unknown };
