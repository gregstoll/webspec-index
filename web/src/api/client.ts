import type { Request, Response, ApiError, QueryResult, SearchResult, ListEntry, SpecEntry, EffectSummaryResult, ExplainEffectsResult, GraphResult } from './types';

export interface WebspecClient {
  request(req: Request): Promise<Response>;
}

// --- WorkerClient ---

type PendingRequest = {
  resolve: (value: Response) => void;
  reject: (reason: unknown) => void;
};

type WorkerMessage = {
  id: string;
  ok: boolean;
  result?: Response;
  error?: ApiError;
};

export class WorkerClient implements WebspecClient {
  private readonly worker: Worker;
  private readonly pending = new Map<string, PendingRequest>();
  private nextId = 0;

  constructor(worker: Worker) {
    this.worker = worker;

    this.worker.addEventListener('message', (event: MessageEvent<WorkerMessage>) => {
      const { id, ok, result, error } = event.data;
      const pending = this.pending.get(id);
      if (!pending) return;
      this.pending.delete(id);
      if (ok && result !== undefined) {
        pending.resolve(result);
      } else {
        pending.reject(error ?? new Error('Worker responded with no result'));
      }
    });

    this.worker.addEventListener('error', () => {
      const entries = [...this.pending.values()];
      this.pending.clear();
      const err = new Error('Worker encountered an unrecoverable error');
      for (const { reject } of entries) reject(err);
    });
  }

  request(req: Request): Promise<Response> {
    const id = String(this.nextId++);
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ id, request: req });
    });
  }
}

// --- Fixtures for MockClient ---

const MOCK_SPECS: SpecEntry[] = [
  { name: 'HTML', base_url: 'https://html.spec.whatwg.org/', provider: 'whatwg', sha: 'abc123', commit_date: '2026-09-15' },
  { name: 'DOM', base_url: 'https://dom.spec.whatwg.org/', provider: 'whatwg', sha: 'def456', commit_date: '2026-09-15' },
  { name: 'FETCH', base_url: 'https://fetch.spec.whatwg.org/', provider: 'whatwg', sha: 'ghi789', commit_date: '2026-09-15' },
];

const MOCK_NAVIGATE: QueryResult = {
  spec: 'HTML',
  sha: 'abc123',
  anchor: 'navigate',
  url: 'https://html.spec.whatwg.org/#navigate',
  title: 'navigate',
  type: 'algorithm',
  content: 'To navigate, given a navigable navigable, a URL url, and other parameters...',
  content_html: '<p><a href="https://example.com/x">ext</a> <a href="#/DOM/concept-tree">int</a> <a href="javascript:alert(1)">js</a></p><ol><li>Let x be y.</li><li>Return x.</li></ol>',
  navigation: {
    parent: { anchor: 'navigation', title: 'Navigation' },
    prev: { anchor: 'beginning-navigation', title: 'Beginning navigation' },
    next: { anchor: 'the-rules-for-choosing-a-browsing-context-given-a-target-name', title: 'Choosing a browsing context' },
    children: [],
  },
  outgoing_refs: [
    { spec: 'DOM', anchor: 'concept-tree', kind: 'step' },
    { spec: 'FETCH', anchor: 'concept-fetch', kind: 'step' },
  ],
  incoming_refs: [
    { spec: 'HTML', anchor: 'the-a-element', kind: 'prose' },
    { spec: 'HTML', anchor: 'form-submission-algorithm', kind: 'step' },
  ],
};

const MOCK_BROWSING: QueryResult = {
  spec: 'HTML',
  sha: 'abc123',
  anchor: 'browsing-the-web',
  url: 'https://html.spec.whatwg.org/#browsing-the-web',
  title: 'Browsing the web',
  number: '7.4',
  type: 'heading',
  navigation: {
    parent: null,
    prev: null,
    next: null,
    children: [
      { anchor: 'navigate', title: 'navigate' },
      { anchor: 'beginning-navigation', title: 'Beginning navigation' },
    ],
  },
  outgoing_refs: [],
  incoming_refs: [],
};

const MOCK_CONCEPT_TREE: QueryResult = {
  spec: 'DOM',
  sha: 'def456',
  anchor: 'concept-tree',
  url: 'https://dom.spec.whatwg.org/#concept-tree',
  title: 'Trees',
  number: '2.1',
  type: 'heading',
  navigation: {
    parent: null,
    prev: null,
    next: null,
    children: [{ anchor: 'concept-tree-order', title: 'Tree order' }],
  },
  outgoing_refs: [],
  incoming_refs: [{ spec: 'HTML', anchor: 'navigate', kind: 'step' }],
};

const MOCK_SEARCH: SearchResult = {
  query: 'navigate',
  results: [
    { spec: 'HTML', anchor: 'navigate', title: 'navigate', type: 'algorithm', snippet: 'To <mark>navigate</mark>, given a navigable...' },
    { spec: 'HTML', anchor: 'beginning-navigation', title: 'Beginning navigation', type: 'algorithm', snippet: '...before <mark>navigating</mark>, the user agent must...' },
    { spec: 'DOM', anchor: 'concept-tree', title: 'Trees', type: 'heading', snippet: 'Objects that participate in a tree <mark>can</mark> also...' },
  ],
};

const MOCK_HTML_HEADINGS: ListEntry[] = [
  { anchor: 'introduction', title: 'Introduction', depth: 2, number: '1' },
  { anchor: 'browsing-the-web', title: 'Browsing the web', depth: 2, number: '7.4' },
  { anchor: 'navigate', title: 'navigate', depth: 3, parent: 'browsing-the-web' },
  { anchor: 'beginning-navigation', title: 'Beginning navigation', depth: 3, parent: 'browsing-the-web', number: '7.4.2' },
];

// --- Graph fixtures ---

export const MOCK_GRAPH_NAVIGATE: GraphResult = {
  root: { spec: 'HTML', anchor: 'navigate' },
  direction: 'both',
  max_depth: 1,
  max_nodes: 60,
  truncated: false,
  nodes: [
    { id: 'HTML#navigate', spec: 'HTML', anchor: 'navigate', title: 'navigate', type: 'algorithm' },
    { id: 'DOM#concept-tree', spec: 'DOM', anchor: 'concept-tree', title: 'Trees' },
    { id: 'FETCH#concept-fetch', spec: 'FETCH', anchor: 'concept-fetch', title: 'fetch' },
    { id: 'HTML#the-a-element', spec: 'HTML', anchor: 'the-a-element', title: 'The a element' },
    { id: 'HTML#form-submission-algorithm', spec: 'HTML', anchor: 'form-submission-algorithm', title: 'Form submission' },
  ],
  edges: [
    { from: 'HTML#navigate', to: 'DOM#concept-tree', kind: 'reference' },
    { from: 'HTML#navigate', to: 'FETCH#concept-fetch', kind: 'reference' },
    { from: 'HTML#the-a-element', to: 'HTML#navigate', kind: 'reference' },
    { from: 'HTML#form-submission-algorithm', to: 'HTML#navigate', kind: 'reference' },
    { from: 'DOM#concept-tree', to: 'HTML#navigate', kind: 'reference' },
  ],
};

// --- Effects fixtures ---

const MOCK_EFFECTS_HTML_NAVIGATE: EffectSummaryResult = {
  schema_version: 1,
  subject: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc123' },
  effects: [
    {
      id: 'effect-1',
      kind: 'fire_event',
      params: { name: 'load', target: null },
      execution: ['inline'],
      location: { spec: 'HTML', anchor: 'navigate', url: 'https://html.spec.whatwg.org/#navigate' },
    },
    {
      id: 'effect-2',
      kind: 'queue_task',
      params: { task: 'networking task' },
      execution: ['separate'],
      location: { spec: 'HTML', anchor: 'navigate', step_path: [2], url: 'https://html.spec.whatwg.org/#navigate' },
    },
  ],
  effects_status: {
    state: 'ready',
    semantics: 'may',
    coverage: 'partial',
    issues: ['unresolved_invocation'],
    omitted: 0,
    analysis_id: 'analysis-1',
  },
  defined_bodies: [],
  issues: [
    { code: 'unresolved_invocation', message: 'call to "fetch" could not be resolved' },
    { code: 'unresolved_invocation', message: 'call to "queue a task" could not be resolved' },
    { code: 'missing_spec', message: 'spec WEBDRIVER is not indexed' },
  ],
};

const MOCK_EFFECTS_EXPLAIN_HTML_NAVIGATE: ExplainEffectsResult = {
  schema_version: 1,
  subject: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc123' },
  effects: MOCK_EFFECTS_HTML_NAVIGATE.effects,
  effects_status: MOCK_EFFECTS_HTML_NAVIGATE.effects_status,
  defined_bodies: [],
  explanations: [
    {
      effect_id: 'effect-1',
      witnesses: [
        {
          hops: [
            {
              from: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc123', step_path: [1] },
              to: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc123' },
              relation: 'invoke',
              site: {
                id: 'site-1',
                subject: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc123' },
                url: 'https://html.spec.whatwg.org/#navigate',
                step_text: 'Fire an event named load at the document',
              },
              context: [],
            },
            {
              from: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc123' },
              to: { spec: 'HTML', anchor: 'concept-event-fire', snapshot_sha: 'abc123' },
              relation: 'invoke',
              site: {
                id: 'site-2',
                subject: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc123' },
                url: 'https://html.spec.whatwg.org/#fire-an-event',
                step_text: 'Dispatch event at target',
              },
              context: [],
            },
          ],
          terminal_evidence: [],
          path_feasibility: 'unchecked',
          issues: [],
        },
      ],
      witnesses_truncated: false,
      issues: [],
    },
  ],
  issues: [],
};

// --- MockClient ---

export class MockClient implements WebspecClient {
  request(req: Request): Promise<Response> {
    return Promise.resolve(this.handle(req));
  }

  private handle(req: Request): Response {
    switch (req.type) {
      case 'specs':
        return { type: 'specs', result: { specs: MOCK_SPECS } };

      case 'exists':
        return { type: 'exists', result: { exists: true, spec: 'HTML', anchor: 'navigate', type: 'algorithm' } };

      case 'query': {
        const target = req.target;
        if (target === 'HTML#navigate' || target === 'https://html.spec.whatwg.org/#navigate') {
          return { type: 'query', result: MOCK_NAVIGATE };
        }
        if (target === 'HTML#browsing-the-web') {
          return { type: 'query', result: MOCK_BROWSING };
        }
        if (target === 'DOM#concept-tree') {
          return { type: 'query', result: MOCK_CONCEPT_TREE };
        }
        // Detect unknown spec vs unknown anchor to return appropriate error codes.
        const hashIdx = target.indexOf('#');
        if (hashIdx !== -1) {
          const spec = target.slice(0, hashIdx);
          const knownSpecs = new Set(MOCK_SPECS.map((s) => s.name));
          if (!knownSpecs.has(spec)) {
            return { type: 'error', code: 'spec_not_indexed', message: `${spec} is not part of this index` };
          }
        }
        return { type: 'error', code: 'not_found', message: `No fixture for target: ${target}` };
      }

      case 'search':
        return { type: 'search', result: { query: req.query, results: MOCK_SEARCH.results } };

      case 'list':
        return {
          type: 'list',
          result: { spec: req.spec, entries: req.spec === 'HTML' ? MOCK_HTML_HEADINGS : [] },
        };

      case 'effects': {
        const { spec, anchor } = req.subject;
        if (spec === 'HTML' && anchor === 'navigate') {
          return { type: 'effects', result: MOCK_EFFECTS_HTML_NAVIGATE };
        }
        return { type: 'error', code: 'subject_not_found', message: 'No prepared analysis for this subject' };
      }

      case 'effects_explain': {
        const { spec, anchor } = req.subject;
        if (spec === 'HTML' && anchor === 'navigate') {
          return { type: 'effects_explain', result: MOCK_EFFECTS_EXPLAIN_HTML_NAVIGATE };
        }
        return { type: 'error', code: 'subject_not_found', message: 'No prepared analysis for this subject' };
      }

      case 'graph': {
        const target = req.target;
        if (target === 'HTML#navigate') {
          return { type: 'graph', result: MOCK_GRAPH_NAVIGATE };
        }
        return { type: 'graph', result: { ...MOCK_GRAPH_NAVIGATE, root: { spec: 'HTML', anchor: target }, nodes: [], edges: [], truncated: false } };
      }

      default:
        return { type: 'error', code: 'not_implemented', message: `MockClient does not implement: ${req.type}` };
    }
  }
}
