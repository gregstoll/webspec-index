import type { DiagramGraph, DiagramNode, DiagramEdge } from './model';
import { mergeGraphs } from './model';
import type { EffectExplanation, EffectSummary, Subject, Witness } from '../api/types';
import { effectLabel } from '../effects/label';

function nodeId(subject: Subject): string {
  if (subject.step_path && subject.step_path.length > 0) {
    return `${subject.spec}#${subject.anchor}@${subject.step_path.join('.')}`;
  }
  return `${subject.spec}#${subject.anchor}`;
}

function nodeKind(subject: Subject, contextSubject: Subject): DiagramNode['kind'] {
  if (
    subject.spec === contextSubject.spec &&
    subject.anchor === contextSubject.anchor &&
    subject.step_path &&
    subject.step_path.length > 0
  ) {
    return 'step';
  }
  if (subject.spec === contextSubject.spec) {
    return 'section';
  }
  return 'external';
}

function nodeHref(subject: Subject): string {
  let href = `#/${subject.spec}/${subject.anchor}`;
  if (subject.step_path && subject.step_path.length > 0) {
    href += `?step=${subject.step_path.join('.')}`;
  }
  return href;
}

function nodeLabel(subject: Subject): string {
  if (subject.step_path && subject.step_path.length > 0) {
    return `step ${subject.step_path.join('.')}`;
  }
  return `${subject.spec}#${subject.anchor}`;
}

function humaniseRelation(relation: string): string {
  return relation.replace(/_/g, ' ');
}

function witnessToGraph(
  witness: Witness,
  effect: EffectSummary,
  subject: Subject,
): DiagramGraph {
  const nodes: DiagramNode[] = [];
  const edges: DiagramEdge[] = [];
  const seenNodes = new Set<string>();

  function addNode(node: DiagramNode): void {
    if (!seenNodes.has(node.id)) {
      nodes.push(node);
      seenNodes.add(node.id);
    }
  }

  for (const hop of witness.hops) {
    const fromId = nodeId(hop.from);
    const toId = nodeId(hop.to);

    const fromSublabel = hop.site.step_text
      ? hop.site.step_text.length > 60
        ? hop.site.step_text.slice(0, 60) + '…'
        : hop.site.step_text
      : undefined;

    addNode({
      id: fromId,
      label: nodeLabel(hop.from),
      sublabel: fromSublabel,
      kind: nodeKind(hop.from, subject),
      href: nodeHref(hop.from),
    });

    addNode({
      id: toId,
      label: nodeLabel(hop.to),
      kind: nodeKind(hop.to, subject),
      href: nodeHref(hop.to),
    });

    let edgeLabel = humaniseRelation(hop.relation);
    if (hop.boundary) {
      edgeLabel += ` · ${hop.boundary.execution}`;
    }

    edges.push({ from: fromId, to: toId, kind: 'call', label: edgeLabel });
  }

  const effectId = `effect:${effect.id}`;
  addNode({
    id: effectId,
    label: effectLabel(effect),
    kind: 'effect',
  });

  if (witness.hops.length > 0) {
    const lastHop = witness.hops[witness.hops.length - 1];
    const lastToId = nodeId(lastHop.to);
    edges.push({ from: lastToId, to: effectId, kind: 'path' });
  }

  return { nodes, edges };
}

export function effectPaths(
  explanation: EffectExplanation,
  effect: EffectSummary,
  subject: Subject,
): DiagramGraph {
  let graph: DiagramGraph = { nodes: [], edges: [] };

  for (const witness of explanation.witnesses) {
    const wGraph = witnessToGraph(witness, effect, subject);
    graph = mergeGraphs(graph, wGraph);
  }

  return graph;
}
