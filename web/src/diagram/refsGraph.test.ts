import { describe, it, expect } from 'vitest';
import { buildRefsGraph } from './refsGraph';
import type { GraphResult } from '../api/types';

const FIXTURE: GraphResult = {
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

describe('buildRefsGraph', () => {
  it('produces one DiagramNode per GraphNode', () => {
    const { graph } = buildRefsGraph(FIXTURE);
    expect(graph.nodes).toHaveLength(5);
  });

  it('sets node label to SPEC#anchor', () => {
    const { graph } = buildRefsGraph(FIXTURE);
    const root = graph.nodes.find((n) => n.id === 'HTML#navigate');
    expect(root?.label).toBe('HTML#navigate');
  });

  it('sets node sublabel to the title', () => {
    const { graph } = buildRefsGraph(FIXTURE);
    const root = graph.nodes.find((n) => n.id === 'HTML#navigate');
    expect(root?.sublabel).toBe('navigate');
  });

  it('sets node href to #/SPEC/anchor', () => {
    const { graph } = buildRefsGraph(FIXTURE);
    const root = graph.nodes.find((n) => n.id === 'HTML#navigate');
    expect(root?.href).toBe('#/HTML/navigate');
  });

  it('sets node kind to section', () => {
    const { graph } = buildRefsGraph(FIXTURE);
    expect(graph.nodes.every((n) => n.kind === 'section')).toBe(true);
  });

  it('produces one DiagramEdge per GraphEdge with kind reference', () => {
    const { graph } = buildRefsGraph(FIXTURE);
    expect(graph.edges).toHaveLength(5);
    expect(graph.edges.every((e) => e.kind === 'reference')).toBe(true);
  });

  it('returns the correct rootId', () => {
    const { rootId } = buildRefsGraph(FIXTURE);
    expect(rootId).toBe('HTML#navigate');
  });

  it('handles a single-node graph with no edges', () => {
    const single: GraphResult = {
      root: { spec: 'DOM', anchor: 'node' },
      direction: 'both',
      max_depth: 1,
      max_nodes: 60,
      truncated: false,
      nodes: [{ id: 'DOM#node', spec: 'DOM', anchor: 'node' }],
      edges: [],
    };
    const { graph, rootId } = buildRefsGraph(single);
    expect(graph.nodes).toHaveLength(1);
    expect(graph.edges).toHaveLength(0);
    expect(rootId).toBe('DOM#node');
  });
});
