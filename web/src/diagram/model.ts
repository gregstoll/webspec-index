export type NodeKind =
  | 'section'
  | 'step'
  | 'branch'
  | 'loop'
  | 'parallel'
  | 'terminal'
  | 'external'
  | 'effect';

export type EdgeKind =
  | 'next'
  | 'then'
  | 'else'
  | 'loop'
  | 'jump'
  | 'call'
  | 'reference'
  | 'path'
  | 'unknown';

export interface DiagramNode {
  id: string;
  label: string;
  sublabel?: string;
  kind: NodeKind;
  href?: string;
  cluster?: string;
}

export interface DiagramEdge {
  from: string;
  to: string;
  kind: EdgeKind;
  label?: string;
}

export interface DiagramGraph {
  nodes: DiagramNode[];
  edges: DiagramEdge[];
}

/** Returns a list of error messages (empty = valid). */
export function validateGraph(g: DiagramGraph): string[] {
  const errors: string[] = [];
  const ids = new Set<string>();

  for (const node of g.nodes) {
    if (ids.has(node.id)) {
      errors.push(`Duplicate node id: ${node.id}`);
    }
    ids.add(node.id);
  }

  for (const edge of g.edges) {
    if (!ids.has(edge.from)) {
      errors.push(`Dangling edge from: ${edge.from}`);
    }
    if (!ids.has(edge.to)) {
      errors.push(`Dangling edge to: ${edge.to}`);
    }
  }

  return errors;
}

/** Union of two graphs. On id collision the first graph's node wins. Identical edges are deduped. */
export function mergeGraphs(a: DiagramGraph, b: DiagramGraph): DiagramGraph {
  const nodeIds = new Set(a.nodes.map((n) => n.id));
  const edgeKeys = new Set(
    a.edges.map((e) => `${e.from}\0${e.to}\0${e.kind}\0${e.label ?? ''}`)
  );

  const nodes = [...a.nodes];
  for (const n of b.nodes) {
    if (!nodeIds.has(n.id)) {
      nodes.push(n);
      nodeIds.add(n.id);
    }
  }

  const edges = [...a.edges];
  for (const e of b.edges) {
    const key = `${e.from}\0${e.to}\0${e.kind}\0${e.label ?? ''}`;
    if (!edgeKeys.has(key)) {
      edges.push(e);
      edgeKeys.add(key);
    }
  }

  return { nodes, edges };
}

/**
 * Monospace label size estimate: 8 px per character, wraps at 32 chars.
 * Line height 18 px.
 */
export function measureLabel(label: string): { width: number; height: number } {
  const LINE_H = 18;
  const CHAR_W = 8;
  const WRAP = 32;

  const words = label.split(' ');
  const lines: string[] = [];
  let current = '';

  for (const word of words) {
    if (current === '') {
      current = word;
    } else if (current.length + 1 + word.length <= WRAP) {
      current += ' ' + word;
    } else {
      lines.push(current);
      current = word;
    }
  }
  if (current !== '') lines.push(current);
  if (lines.length === 0) lines.push('');

  const maxLen = Math.max(...lines.map((l) => l.length));
  return {
    width: maxLen * CHAR_W,
    height: lines.length * LINE_H,
  };
}
