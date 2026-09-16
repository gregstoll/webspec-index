import { describe, it, expect } from 'vitest';
import { buildFlowGraph } from './flowGraph';
import { validateGraph } from './model';
import { MOCK_FLOW_NAVIGATE } from '../api/client';

// MOCK_FLOW_NAVIGATE has 6 nodes (5 non-external + 1 external) and 6 edges (5 non-call + 1 call).

describe('buildFlowGraph (default: callsAsNodes=false)', () => {
  it('drops external nodes', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    expect(graph.nodes.find((n) => n.kind === 'external')).toBeUndefined();
  });

  it('drops call edges', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    expect(graph.edges.find((e) => e.kind === 'call')).toBeUndefined();
  });

  it('produces one node per non-external FlowNode', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const nonExternal = MOCK_FLOW_NAVIGATE.nodes.filter((n) => n.kind !== 'external');
    expect(graph.nodes).toHaveLength(nonExternal.length);
  });

  it('produces one edge per non-call FlowEdge', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const nonCall = MOCK_FLOW_NAVIGATE.edges.filter((e) => e.kind !== 'call');
    expect(graph.edges).toHaveLength(nonCall.length);
  });

  it('folds calls into sublabel with arrow prefix', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    // Node 1 has calls: [{ spec: 'FETCH', anchor: 'concept-fetch' }]
    const node = graph.nodes.find((n) => n.id === '1');
    expect(node?.sublabel).toContain('→ FETCH#concept-fetch');
  });

  it('step nodes without calls have no sublabel', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const node = graph.nodes.find((n) => n.id === '3');
    expect(node?.sublabel).toBeUndefined();
  });

  it('+N overflow when calls exceed 3', () => {
    const result = {
      ...MOCK_FLOW_NAVIGATE,
      nodes: [
        {
          id: 's',
          kind: 'step' as const,
          text: 'Multi-call step',
          calls: [
            { spec: 'A', anchor: 'a1' },
            { spec: 'B', anchor: 'b1' },
            { spec: 'C', anchor: 'c1' },
            { spec: 'D', anchor: 'd1' },
            { spec: 'E', anchor: 'e1' },
          ],
        },
      ],
      edges: [],
      issues: [],
    };
    const graph = buildFlowGraph(result);
    const node = graph.nodes.find((n) => n.id === 's');
    expect(node?.sublabel).toContain('→ A#a1');
    expect(node?.sublabel).toContain('→ B#b1');
    expect(node?.sublabel).toContain('→ C#c1');
    expect(node?.sublabel).toContain('+2 more');
    expect(node?.sublabel).not.toContain('→ D#d1');
  });

  it('produced graph passes validateGraph', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    expect(validateGraph(graph)).toHaveLength(0);
  });
});

describe('buildFlowGraph (callsAsNodes: true)', () => {
  it('produces one DiagramNode per FlowNode', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    expect(graph.nodes).toHaveLength(MOCK_FLOW_NAVIGATE.nodes.length);
  });

  it('maps step node kind to step', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const node = graph.nodes.find((n) => n.id === '1');
    expect(node?.kind).toBe('step');
  });

  it('uses step text as label for non-external nodes', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const node = graph.nodes.find((n) => n.id === '1');
    expect(node?.label).toBe('Let resource be the result of fetching url.');
  });

  it('maps branch node kind to branch', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const node = graph.nodes.find((n) => n.id === '2');
    expect(node?.kind).toBe('branch');
  });

  it('maps terminal node kind to terminal', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const node = graph.nodes.find((n) => n.id === '5');
    expect(node?.kind).toBe('terminal');
  });

  it('maps external node kind to external', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const node = graph.nodes.find((n) => n.id === 'FETCH#concept-fetch');
    expect(node?.kind).toBe('external');
  });

  it('sets external node label to SPEC#anchor id', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const node = graph.nodes.find((n) => n.id === 'FETCH#concept-fetch');
    expect(node?.label).toBe('FETCH#concept-fetch');
  });

  it('sets external node href to #/SPEC/anchor', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const node = graph.nodes.find((n) => n.id === 'FETCH#concept-fetch');
    expect(node?.href).toBe('#/FETCH/concept-fetch');
  });

  it('step nodes have no href', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const node = graph.nodes.find((n) => n.id === '1');
    expect(node?.href).toBeUndefined();
  });

  it('produces one DiagramEdge per FlowEdge', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    expect(graph.edges).toHaveLength(MOCK_FLOW_NAVIGATE.edges.length);
  });

  it('maps next edge kind to next', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const edge = graph.edges.find((e) => e.from === '1' && e.to === '2');
    expect(edge?.kind).toBe('next');
  });

  it('maps then edge kind to then', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const edge = graph.edges.find((e) => e.from === '2' && e.to === '3');
    expect(edge?.kind).toBe('then');
  });

  it('maps else edge kind to else', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const edge = graph.edges.find((e) => e.from === '2' && e.to === '4');
    expect(edge?.kind).toBe('else');
  });

  it('maps call edge kind to call', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    const edge = graph.edges.find((e) => e.from === '1' && e.to === 'FETCH#concept-fetch');
    expect(edge?.kind).toBe('call');
  });

  it('produced graph passes validateGraph', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE, { callsAsNodes: true });
    expect(validateGraph(graph)).toHaveLength(0);
  });
});
