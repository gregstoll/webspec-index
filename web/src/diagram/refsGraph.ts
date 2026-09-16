import type { GraphResult } from '../api/types';
import type { DiagramGraph, DiagramNode, DiagramEdge } from './model';

/**
 * Convert a GraphResult from the `graph` API request into a DiagramGraph
 * suitable for the Diagram component.
 *
 * The root node (matching rootId = `${result.root.spec}#${result.root.anchor}`)
 * is tagged for use as `selectedId` — the caller receives it via the second
 * return value.
 */
export function buildRefsGraph(result: GraphResult): { graph: DiagramGraph; rootId: string } {
  const rootId = `${result.root.spec}#${result.root.anchor}`;

  const nodes: DiagramNode[] = result.nodes.map((n) => ({
    id: n.id,
    label: n.id,
    sublabel: n.title,
    kind: 'section',
    href: `#/${n.spec}/${n.anchor}`,
  }));

  const edges: DiagramEdge[] = result.edges.map((e) => ({
    from: e.from,
    to: e.to,
    kind: 'reference',
  }));

  return { graph: { nodes, edges }, rootId };
}
