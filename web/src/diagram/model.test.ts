import { describe, it, expect } from 'vitest';
import {
  validateGraph,
  mergeGraphs,
  measureLabel,
} from './model';
import type { DiagramGraph } from './model';

describe('validateGraph', () => {
  it('returns empty array for a valid graph', () => {
    const g: DiagramGraph = {
      nodes: [
        { id: 'a', label: 'A', kind: 'section' },
        { id: 'b', label: 'B', kind: 'step' },
      ],
      edges: [{ from: 'a', to: 'b', kind: 'next' }],
    };
    expect(validateGraph(g)).toEqual([]);
  });

  it('flags a duplicate node id', () => {
    const g: DiagramGraph = {
      nodes: [
        { id: 'a', label: 'A', kind: 'section' },
        { id: 'a', label: 'A2', kind: 'step' },
      ],
      edges: [],
    };
    const errors = validateGraph(g);
    expect(errors.some((e) => e.includes('Duplicate') && e.includes('a'))).toBe(true);
  });

  it('flags a dangling edge (from)', () => {
    const g: DiagramGraph = {
      nodes: [{ id: 'a', label: 'A', kind: 'section' }],
      edges: [{ from: 'missing', to: 'a', kind: 'next' }],
    };
    const errors = validateGraph(g);
    expect(errors.some((e) => e.includes('Dangling') && e.includes('missing'))).toBe(true);
  });

  it('flags a dangling edge (to)', () => {
    const g: DiagramGraph = {
      nodes: [{ id: 'a', label: 'A', kind: 'section' }],
      edges: [{ from: 'a', to: 'gone', kind: 'next' }],
    };
    const errors = validateGraph(g);
    expect(errors.some((e) => e.includes('Dangling') && e.includes('gone'))).toBe(true);
  });
});

describe('mergeGraphs', () => {
  it('keeps the first node on id collision', () => {
    const a: DiagramGraph = {
      nodes: [{ id: 'x', label: 'A', kind: 'section' }],
      edges: [],
    };
    const b: DiagramGraph = {
      nodes: [{ id: 'x', label: 'B', kind: 'step' }],
      edges: [],
    };
    const merged = mergeGraphs(a, b);
    expect(merged.nodes).toHaveLength(1);
    expect(merged.nodes[0].label).toBe('A');
  });

  it('dedupes identical edges', () => {
    const edge = { from: 'a', to: 'b', kind: 'next' as const };
    const a: DiagramGraph = {
      nodes: [
        { id: 'a', label: 'A', kind: 'section' },
        { id: 'b', label: 'B', kind: 'step' },
      ],
      edges: [edge],
    };
    const b: DiagramGraph = {
      nodes: [],
      edges: [edge],
    };
    const merged = mergeGraphs(a, b);
    expect(merged.edges).toHaveLength(1);
  });

  it('includes nodes from b that are not in a', () => {
    const a: DiagramGraph = {
      nodes: [{ id: 'a', label: 'A', kind: 'section' }],
      edges: [],
    };
    const b: DiagramGraph = {
      nodes: [{ id: 'c', label: 'C', kind: 'external' }],
      edges: [],
    };
    const merged = mergeGraphs(a, b);
    expect(merged.nodes).toHaveLength(2);
  });
});

describe('measureLabel', () => {
  it('gives a positive size for a non-empty label', () => {
    const { width, height } = measureLabel('Hello world');
    expect(width).toBeGreaterThan(0);
    expect(height).toBeGreaterThan(0);
  });

  it('wraps long labels', () => {
    const short = measureLabel('Hello');
    const long = measureLabel('a '.repeat(40).trim());
    expect(long.height).toBeGreaterThan(short.height);
  });
});
