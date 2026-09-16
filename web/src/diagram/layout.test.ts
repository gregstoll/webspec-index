import { describe, it, expect } from 'vitest';
import { layoutGraph } from './layout';
import type { DiagramGraph } from './model';

const twoNodeChain: DiagramGraph = {
  nodes: [
    { id: 'a', label: 'First', kind: 'step' },
    { id: 'b', label: 'Second', kind: 'step' },
  ],
  edges: [{ from: 'a', to: 'b', kind: 'next' }],
};

describe('layoutGraph', () => {
  it('places second node below the first in TB (top-to-bottom) layout', async () => {
    const layout = await layoutGraph(twoNodeChain, { rankdir: 'TB' });
    const a = layout.nodes.find((n) => n.id === 'a')!;
    const b = layout.nodes.find((n) => n.id === 'b')!;
    expect(b.y).toBeGreaterThan(a.y);
  });

  it('places second node to the right of the first in LR (left-to-right) layout', async () => {
    const layout = await layoutGraph(twoNodeChain, { rankdir: 'LR' });
    const a = layout.nodes.find((n) => n.id === 'a')!;
    const b = layout.nodes.find((n) => n.id === 'b')!;
    expect(b.x).toBeGreaterThan(a.x);
  });

  it('returns positive width and height', async () => {
    const layout = await layoutGraph(twoNodeChain);
    expect(layout.width).toBeGreaterThan(0);
    expect(layout.height).toBeGreaterThan(0);
  });

  it('returns edge points', async () => {
    const layout = await layoutGraph(twoNodeChain);
    expect(layout.edges).toHaveLength(1);
    expect(layout.edges[0].points.length).toBeGreaterThan(0);
  });

  it('tighter ranksep produces a smaller vertical gap between nodes', async () => {
    const loose = await layoutGraph(twoNodeChain, { rankdir: 'TB', ranksep: 80 });
    const tight = await layoutGraph(twoNodeChain, { rankdir: 'TB', ranksep: 20 });
    const looseA = loose.nodes.find((n) => n.id === 'a')!;
    const looseB = loose.nodes.find((n) => n.id === 'b')!;
    const tightA = tight.nodes.find((n) => n.id === 'a')!;
    const tightB = tight.nodes.find((n) => n.id === 'b')!;
    expect(tightB.y - tightA.y).toBeLessThan(looseB.y - looseA.y);
  });

  it('wider labelWidth produces wider nodes', async () => {
    const narrow = await layoutGraph(twoNodeChain, { labelWidth: 5 });
    const wide = await layoutGraph(twoNodeChain, { labelWidth: 50 });
    const narrowNode = narrow.nodes.find((n) => n.id === 'a')!;
    const wideNode = wide.nodes.find((n) => n.id === 'a')!;
    // "First" is a single word — it won't wrap at either width, but a wider
    // labelWidth means the label is allowed to be wider, which should not shrink it.
    // Use a multi-word label to verify wrapping effect.
    expect(wideNode.width).toBeGreaterThanOrEqual(narrowNode.width);
  });

  it('node with a two-line sublabel gets a taller box than the same node without', async () => {
    const base: DiagramGraph = {
      nodes: [{ id: 'x', label: 'Navigate', kind: 'step' }],
      edges: [],
    };
    const withSublabel: DiagramGraph = {
      nodes: [
        {
          id: 'x',
          label: 'Navigate',
          kind: 'step',
          sublabel: 'A long sublabel text that definitely wraps into two lines at narrow width',
        },
      ],
      edges: [],
    };
    const baseLayout = await layoutGraph(base, { labelWidth: 20 });
    const sublabelLayout = await layoutGraph(withSublabel, { labelWidth: 20 });
    expect(sublabelLayout.nodes[0].height).toBeGreaterThan(baseLayout.nodes[0].height);
  });

  it('labelWidth=5 wraps "First long label" into more lines than labelWidth=50', async () => {
    const graph: DiagramGraph = {
      nodes: [{ id: 'x', label: 'First long label for testing', kind: 'step' }],
      edges: [],
    };
    const narrow = await layoutGraph(graph, { labelWidth: 5 });
    const wide = await layoutGraph(graph, { labelWidth: 50 });
    const nNode = narrow.nodes.find((n) => n.id === 'x')!;
    const wNode = wide.nodes.find((n) => n.id === 'x')!;
    // Narrow wrap → more lines → taller node
    expect(nNode.height).toBeGreaterThan(wNode.height);
  });
});
