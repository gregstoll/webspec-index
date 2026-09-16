import { describe, it, expect } from 'vitest';
import { traceGraph } from './traceGraph';
import type { TraceEntry } from '../trace/store';

const noTitles = new Map();

describe('traceGraph', () => {
  it('returns empty graph for empty entries', () => {
    const g = traceGraph([], noTitles);
    expect(g.nodes).toHaveLength(0);
    expect(g.edges).toHaveLength(0);
  });

  it('two entries on the same section share one section node', () => {
    const entries: TraceEntry[] = [
      { id: '1', spec: 'HTML', anchor: 'navigate' },
      { id: '2', spec: 'HTML', anchor: 'navigate' },
    ];
    const g = traceGraph(entries, noTitles);
    expect(g.nodes).toHaveLength(1);
    expect(g.nodes[0].id).toBe('HTML#navigate');
    expect(g.nodes[0].kind).toBe('section');
  });

  it('two different sections produce two nodes and one path edge', () => {
    const entries: TraceEntry[] = [
      { id: '1', spec: 'HTML', anchor: 'navigate' },
      { id: '2', spec: 'DOM', anchor: 'concept-tree' },
    ];
    const g = traceGraph(entries, noTitles);
    expect(g.nodes).toHaveLength(2);
    expect(g.edges).toHaveLength(1);
    expect(g.edges[0]).toMatchObject({ from: 'HTML#navigate', to: 'DOM#concept-tree', kind: 'path' });
  });

  it('a step entry is one node naming the algorithm and the step, with no separate section node', () => {
    const entries: TraceEntry[] = [
      { id: '1', spec: 'HTML', anchor: 'navigate', step_path: [3, 1] },
    ];
    const g = traceGraph(entries, noTitles);
    expect(g.nodes).toHaveLength(1);
    expect(g.nodes[0]).toMatchObject({
      id: 'HTML#navigate@3.1',
      kind: 'step',
      label: 'HTML#navigate · step 3.1',
      href: '#/HTML/navigate?step=3.1',
    });
    expect(g.edges).toHaveLength(0);
  });

  it('steps of different algorithms link directly step to step', () => {
    const entries: TraceEntry[] = [
      { id: '1', spec: 'HTML', anchor: 'navigate', step_path: [24, 9, 1] },
      { id: '2', spec: 'HTML', anchor: 'attempt-to-populate-the-history-entrys-document', step_path: [5] },
      { id: '3', spec: 'HTML', anchor: 'attempt-to-populate-the-history-entrys-document', step_path: [5] },
    ];
    const g = traceGraph(entries, noTitles);
    expect(g.nodes).toHaveLength(2);
    expect(g.edges).toEqual([
      {
        from: 'HTML#navigate@24.9.1',
        to: 'HTML#attempt-to-populate-the-history-entrys-document@5',
        kind: 'path',
      },
    ]);
  });

  it('consecutive entries get one path edge each, self-loops skipped', () => {
    const entries: TraceEntry[] = [
      { id: '1', spec: 'HTML', anchor: 'navigate' },
      { id: '2', spec: 'HTML', anchor: 'navigate' },
      { id: '3', spec: 'DOM', anchor: 'concept-tree' },
      { id: '4', spec: 'DOM', anchor: 'concept-tree' },
    ];
    const g = traceGraph(entries, noTitles);
    const pathEdges = g.edges.filter((e) => e.kind === 'path');
    expect(pathEdges).toHaveLength(1);
    expect(pathEdges[0]).toMatchObject({ from: 'HTML#navigate', to: 'DOM#concept-tree' });
  });

  it('uses title from cache as sublabel when available', () => {
    const titles = new Map([
      ['HTML#navigate', { title: 'Navigation', url: '#/HTML/navigate' }],
    ]);
    const entries: TraceEntry[] = [{ id: '1', spec: 'HTML', anchor: 'navigate' }];
    const g = traceGraph(entries, titles);
    expect(g.nodes[0].sublabel).toBe('Navigation');
  });

  it('does not set sublabel when title equals the key', () => {
    const titles = new Map([
      ['HTML#navigate', { title: 'HTML#navigate', url: '#/HTML/navigate' }],
    ]);
    const entries: TraceEntry[] = [{ id: '1', spec: 'HTML', anchor: 'navigate' }];
    const g = traceGraph(entries, titles);
    expect(g.nodes[0].sublabel).toBeUndefined();
  });

  it('note on a section entry becomes the section node sublabel', () => {
    const entries: TraceEntry[] = [
      { id: '1', spec: 'HTML', anchor: 'navigate', note: 'entry point' },
    ];
    const g = traceGraph(entries, noTitles);
    expect(g.nodes[0].sublabel).toBe('entry point');
  });

  it('note on a step entry becomes the step node sublabel', () => {
    const entries: TraceEntry[] = [
      { id: '1', spec: 'HTML', anchor: 'navigate', step_path: [1], note: 'key step' },
    ];
    const g = traceGraph(entries, noTitles);
    const step = g.nodes.find((n) => n.kind === 'step');
    expect(step!.sublabel).toBe('key step');
  });

  it('section node has correct href', () => {
    const entries: TraceEntry[] = [{ id: '1', spec: 'HTML', anchor: 'navigate' }];
    const g = traceGraph(entries, noTitles);
    expect(g.nodes[0].href).toBe('#/HTML/navigate');
  });

  it('three-section chain produces two path edges', () => {
    const entries: TraceEntry[] = [
      { id: '1', spec: 'A', anchor: 'x' },
      { id: '2', spec: 'B', anchor: 'y' },
      { id: '3', spec: 'C', anchor: 'z' },
    ];
    const g = traceGraph(entries, noTitles);
    expect(g.edges).toHaveLength(2);
  });
});
