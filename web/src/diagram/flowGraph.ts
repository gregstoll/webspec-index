import type { FlowResult } from '../api/types';
import type { DiagramGraph, DiagramNode, DiagramEdge, NodeKind, EdgeKind } from './model';

/**
 * Convert a FlowResult from the `flow` API request into a DiagramGraph
 * suitable for the Diagram component.
 *
 * Node kinds map 1:1 from Rust to DiagramNode kind. External nodes carry
 * an href to their spec page; all other nodes are selectable steps.
 * Edge kinds map 1:1.
 */
export function buildFlowGraph(result: FlowResult): DiagramGraph {
  const nodes: DiagramNode[] = result.nodes.map((node): DiagramNode => {
    if (node.kind === 'external') {
      const [spec, anchor] = node.id.split('#');
      return {
        id: node.id,
        label: node.id,
        kind: 'external' as NodeKind,
        href: `#/${spec}/${anchor}`,
      };
    }
    return {
      id: node.id,
      label: node.text,
      kind: node.kind as NodeKind,
    };
  });

  const edges: DiagramEdge[] = result.edges.map((edge): DiagramEdge => ({
    from: edge.from,
    to: edge.to,
    kind: edge.kind as EdgeKind,
    ...(edge.label !== undefined ? { label: edge.label } : {}),
  }));

  return { nodes, edges };
}
