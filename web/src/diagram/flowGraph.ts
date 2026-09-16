import type { FlowResult } from '../api/types';
import type { DiagramGraph, DiagramNode, DiagramEdge, NodeKind, EdgeKind } from './model';

export interface BuildFlowGraphOptions {
  /**
   * When true, external call targets appear as nodes connected by `call` edges —
   * today's behaviour. When false (default), external nodes and call edges are
   * dropped; instead each step node receives a `sublabel` listing the call
   * targets it makes (max 3, then `+N more`).
   */
  callsAsNodes?: boolean;
}

/**
 * Convert a FlowResult from the `flow` API request into a DiagramGraph
 * suitable for the Diagram component.
 *
 * Node kinds map 1:1 from Rust to DiagramNode kind. External nodes carry
 * an href to their spec page; all other nodes are selectable steps.
 * Edge kinds map 1:1.
 */
export function buildFlowGraph(
  result: FlowResult,
  opts: BuildFlowGraphOptions = {}
): DiagramGraph {
  const callsAsNodes = opts.callsAsNodes ?? false;

  const nodes: DiagramNode[] = [];
  const edges: DiagramEdge[] = [];

  for (const node of result.nodes) {
    if (node.kind === 'external') {
      if (callsAsNodes) {
        const [spec, anchor] = node.id.split('#');
        nodes.push({
          id: node.id,
          label: node.id,
          kind: 'external' as NodeKind,
          href: `#/${spec}/${anchor}`,
        });
      }
      // When callsAsNodes is false, skip external nodes entirely.
      continue;
    }

    // Non-external node: build sublabel from calls when callsAsNodes is false.
    let sublabel: string | undefined;
    if (!callsAsNodes && node.calls && node.calls.length > 0) {
      const MAX = 3;
      const labels = node.calls.slice(0, MAX).map((c) => `→ ${c.spec}#${c.anchor}`);
      if (node.calls.length > MAX) {
        labels.push(`+${node.calls.length - MAX} more`);
      }
      sublabel = labels.join('\n');
    }

    nodes.push({
      id: node.id,
      label: node.text,
      kind: node.kind as NodeKind,
      ...(sublabel !== undefined ? { sublabel } : {}),
    });
  }

  for (const edge of result.edges) {
    if (!callsAsNodes && edge.kind === 'call') {
      // Drop call edges when folding calls into sublabels.
      continue;
    }
    edges.push({
      from: edge.from,
      to: edge.to,
      kind: edge.kind as EdgeKind,
      ...(edge.label !== undefined ? { label: edge.label } : {}),
    });
  }

  return { nodes, edges };
}
