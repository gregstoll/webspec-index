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
});
