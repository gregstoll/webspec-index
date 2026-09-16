import { describe, it, expect } from 'vitest';
import { buildFlowGraph } from './flowGraph';
import { validateGraph } from './model';
import { MOCK_FLOW_NAVIGATE } from '../api/client';

describe('buildFlowGraph', () => {
  it('produces one DiagramNode per FlowNode', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    expect(graph.nodes).toHaveLength(MOCK_FLOW_NAVIGATE.nodes.length);
  });

  it('maps step node kind to step', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const node = graph.nodes.find((n) => n.id === '1');
    expect(node?.kind).toBe('step');
  });

  it('uses step text as label for non-external nodes', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const node = graph.nodes.find((n) => n.id === '1');
    expect(node?.label).toBe('Let resource be the result of fetching url.');
  });

  it('maps branch node kind to branch', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const node = graph.nodes.find((n) => n.id === '2');
    expect(node?.kind).toBe('branch');
  });

  it('maps terminal node kind to terminal', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const node = graph.nodes.find((n) => n.id === '5');
    expect(node?.kind).toBe('terminal');
  });

  it('maps external node kind to external', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const node = graph.nodes.find((n) => n.id === 'FETCH#concept-fetch');
    expect(node?.kind).toBe('external');
  });

  it('sets external node label to SPEC#anchor id', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const node = graph.nodes.find((n) => n.id === 'FETCH#concept-fetch');
    expect(node?.label).toBe('FETCH#concept-fetch');
  });

  it('sets external node href to #/SPEC/anchor', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const node = graph.nodes.find((n) => n.id === 'FETCH#concept-fetch');
    expect(node?.href).toBe('#/FETCH/concept-fetch');
  });

  it('step nodes have no href', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const node = graph.nodes.find((n) => n.id === '1');
    expect(node?.href).toBeUndefined();
  });

  it('produces one DiagramEdge per FlowEdge', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    expect(graph.edges).toHaveLength(MOCK_FLOW_NAVIGATE.edges.length);
  });

  it('maps next edge kind to next', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const edge = graph.edges.find((e) => e.from === '1' && e.to === '2');
    expect(edge?.kind).toBe('next');
  });

  it('maps then edge kind to then', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const edge = graph.edges.find((e) => e.from === '2' && e.to === '3');
    expect(edge?.kind).toBe('then');
  });

  it('maps else edge kind to else', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const edge = graph.edges.find((e) => e.from === '2' && e.to === '4');
    expect(edge?.kind).toBe('else');
  });

  it('maps call edge kind to call', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    const edge = graph.edges.find((e) => e.from === '1' && e.to === 'FETCH#concept-fetch');
    expect(edge?.kind).toBe('call');
  });

  it('produced graph passes validateGraph', () => {
    const graph = buildFlowGraph(MOCK_FLOW_NAVIGATE);
    expect(validateGraph(graph)).toHaveLength(0);
  });
});
