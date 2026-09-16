import type { DiagramGraph, DiagramNode, DiagramEdge } from './model';

/** Sanitise a node id for use as a Mermaid identifier (letters, digits, underscores). */
function sanitiseId(id: string): string {
  return id.replace(/[^a-zA-Z0-9_]/g, '_');
}

/** Escape a label string for Mermaid double-quoted labels. */
function escapeLabel(label: string): string {
  return label.replace(/"/g, '#quot;').replace(/\n/g, ' ');
}

/** Format a node in Mermaid syntax with shape determined by kind. */
function nodeDecl(node: DiagramNode): string {
  const mid = sanitiseId(node.id);
  const label = escapeLabel(node.label);

  switch (node.kind) {
    case 'branch':
      return `${mid}{"${label}"}`;
    case 'terminal':
      return `${mid}(["${label}"])`;
    case 'external':
      return `${mid}[["${label}"]]`;
    case 'loop':
      return `${mid}(("${label}"))`;
    case 'parallel':
      return `${mid}[/"${label}"\\]`;
    case 'effect':
      return `${mid}>"${label}"]`;
    default:
      // section, step, unknown kinds → rectangle
      return `${mid}["${label}"]`;
  }
}

/** Format an edge in Mermaid syntax. */
function edgeDecl(edge: DiagramEdge): string {
  const from = sanitiseId(edge.from);
  const to = sanitiseId(edge.to);

  if (edge.kind === 'unknown') {
    if (edge.label != null) {
      return `${from} -. "${escapeLabel(edge.label)}" .-> ${to}`;
    }
    return `${from} -.-> ${to}`;
  }
  if (edge.label != null) {
    return `${from} -- "${escapeLabel(edge.label)}" --> ${to}`;
  }
  return `${from} --> ${to}`;
}

/** Convert a DiagramGraph to a Mermaid flowchart string. */
export function toMermaid(g: DiagramGraph): string {
  const lines: string[] = ['flowchart TD'];

  for (const node of g.nodes) {
    lines.push(`  ${nodeDecl(node)}`);
  }

  for (const edge of g.edges) {
    lines.push(`  ${edgeDecl(edge)}`);
  }

  return lines.join('\n');
}
