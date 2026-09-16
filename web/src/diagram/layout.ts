import type { DiagramGraph, DiagramNode, DiagramEdge } from './model';
import { measureLabel } from './model';

export interface LayoutNode extends DiagramNode {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface LayoutEdge extends DiagramEdge {
  points: { x: number; y: number }[];
}

export interface Layout {
  nodes: LayoutNode[];
  edges: LayoutEdge[];
  width: number;
  height: number;
}

const PAD_X = 24;
const PAD_Y = 16;

/**
 * Lay out a DiagramGraph using dagre. The dagre module is loaded lazily so
 * Vite emits it as a separate chunk.
 */
export async function layoutGraph(
  g: DiagramGraph,
  opts?: { rankdir?: 'TB' | 'LR' }
): Promise<Layout> {
  const dagre = await import('@dagrejs/dagre');
  const graph = new dagre.graphlib.Graph();
  graph.setDefaultEdgeLabel(() => ({}));
  graph.setGraph({ rankdir: opts?.rankdir ?? 'TB', nodesep: 40, ranksep: 60 });

  for (const node of g.nodes) {
    const { width, height } = measureLabel(node.label);
    graph.setNode(node.id, {
      width: width + PAD_X * 2,
      height: height + PAD_Y * 2,
    });
  }

  for (const edge of g.edges) {
    graph.setEdge(edge.from, edge.to);
  }

  dagre.layout(graph);

  const layoutNodes: LayoutNode[] = g.nodes.map((node) => {
    const n = graph.node(node.id);
    return {
      ...node,
      x: n.x,
      y: n.y,
      width: n.width,
      height: n.height,
    };
  });

  const layoutEdges: LayoutEdge[] = g.edges.map((edge) => {
    const e = graph.edge(edge.from, edge.to);
    const points: { x: number; y: number }[] = e?.points ?? [
      { x: 0, y: 0 },
      { x: 0, y: 0 },
    ];
    return { ...edge, points };
  });

  const graphObj = graph.graph();
  return {
    nodes: layoutNodes,
    edges: layoutEdges,
    width: graphObj.width ?? 0,
    height: graphObj.height ?? 0,
  };
}
