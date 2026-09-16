import type { TraceEntry } from '../trace/store';
import type { TitleEntry } from '../trace/titles';
import type { DiagramGraph, DiagramNode, DiagramEdge } from './model';
import { routeToHash } from '../router';

function sectionId(spec: string, anchor: string): string {
  return `${spec}#${anchor}`;
}

function stepId(spec: string, anchor: string, step_path: number[]): string {
  return `${spec}#${anchor}@${step_path.join('.')}`;
}

function sectionHref(spec: string, anchor: string): string {
  return routeToHash({ kind: 'section', spec, anchor });
}

function stepHref(spec: string, anchor: string, step_path: number[]): string {
  return routeToHash({ kind: 'section', spec, anchor, step: step_path });
}

/**
 * Build a DiagramGraph from an ordered list of trace entries.
 *
 * - One `section` node per distinct SPEC#anchor.
 * - An entry with step_path adds a `step` node clustered under its section,
 *   plus a path edge section → step (deduped).
 * - Consecutive entries are linked with a `path` edge (self-loops skipped).
 * - Notes on section entries become the node sublabel (last note wins on collision).
 */
export function traceGraph(
  entries: readonly TraceEntry[],
  titles: Map<string, TitleEntry>,
): DiagramGraph {
  const nodes = new Map<string, DiagramNode>();
  const edgeKeys = new Set<string>();
  const edges: DiagramEdge[] = [];

  function addEdge(from: string, to: string): void {
    if (from === to) return;
    const key = `${from}\0${to}`;
    if (edgeKeys.has(key)) return;
    edgeKeys.add(key);
    edges.push({ from, to, kind: 'path' });
  }

  function ensureSection(spec: string, anchor: string, note?: string): string {
    const id = sectionId(spec, anchor);
    if (!nodes.has(id)) {
      const cacheKey = `${spec}#${anchor}`;
      const cached = titles.get(cacheKey);
      nodes.set(id, {
        id,
        label: cacheKey,
        sublabel: cached?.title !== cacheKey ? cached?.title : undefined,
        kind: 'section',
        href: sectionHref(spec, anchor),
      });
    }
    if (note !== undefined) {
      const existing = nodes.get(id)!;
      nodes.set(id, { ...existing, sublabel: note });
    }
    return id;
  }

  const nodeIdForEntry: string[] = [];

  for (const entry of entries) {
    const secId = ensureSection(entry.spec, entry.anchor, entry.step_path ? undefined : entry.note);

    if (entry.step_path && entry.step_path.length > 0) {
      const sId = stepId(entry.spec, entry.anchor, entry.step_path);
      if (!nodes.has(sId)) {
        nodes.set(sId, {
          id: sId,
          label: `step ${entry.step_path.join('.')}`,
          sublabel: entry.note,
          kind: 'step',
          href: stepHref(entry.spec, entry.anchor, entry.step_path),
          cluster: secId,
        });
      } else if (entry.note !== undefined) {
        const existing = nodes.get(sId)!;
        nodes.set(sId, { ...existing, sublabel: entry.note });
      }
      addEdge(secId, sId);
      nodeIdForEntry.push(sId);
    } else {
      nodeIdForEntry.push(secId);
    }
  }

  for (let i = 0; i + 1 < nodeIdForEntry.length; i++) {
    addEdge(nodeIdForEntry[i], nodeIdForEntry[i + 1]);
  }

  return { nodes: [...nodes.values()], edges };
}
