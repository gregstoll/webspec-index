import type { TraceEntry } from '../trace/store';
import type { TitleEntry } from '../trace/titles';
import type { DiagramGraph, DiagramNode, DiagramEdge } from './model';
import { routeToHash } from '../router';

function entryId(entry: TraceEntry): string {
  const base = `${entry.spec}#${entry.anchor}`;
  return entry.step_path && entry.step_path.length > 0 ? `${base}@${entry.step_path.join('.')}` : base;
}

function entryHref(entry: TraceEntry): string {
  return entry.step_path && entry.step_path.length > 0
    ? routeToHash({ kind: 'section', spec: entry.spec, anchor: entry.anchor, step: entry.step_path })
    : routeToHash({ kind: 'section', spec: entry.spec, anchor: entry.anchor });
}

/**
 * Build a DiagramGraph from an ordered list of trace entries.
 *
 * - One node per distinct entry target: a section (`SPEC#anchor`) or a step of it
 *   (`SPEC#anchor@3.1`, labelled "SPEC#anchor · step 3.1"). Steps are not hung
 *   below a separate section node; the path runs directly from step to step.
 * - Consecutive entries are linked with a `path` edge (self-loops skipped, deduped).
 * - The note of an entry becomes its node's sublabel (last note wins); a section
 *   node without a note shows the cached title instead.
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

  const ids: string[] = [];
  for (const entry of entries) {
    const id = entryId(entry);
    const key = `${entry.spec}#${entry.anchor}`;
    const isStep = entry.step_path !== undefined && entry.step_path.length > 0;
    if (!nodes.has(id)) {
      const cached = titles.get(key);
      const title = cached?.title !== key ? cached?.title : undefined;
      nodes.set(id, {
        id,
        label: isStep ? `${key} · step ${entry.step_path!.join('.')}` : key,
        sublabel: entry.note ?? title,
        kind: isStep ? 'step' : 'section',
        href: entryHref(entry),
      });
    } else if (entry.note !== undefined) {
      nodes.set(id, { ...nodes.get(id)!, sublabel: entry.note });
    }
    ids.push(id);
  }

  for (let i = 0; i + 1 < ids.length; i++) addEdge(ids[i], ids[i + 1]);

  return { nodes: [...nodes.values()], edges };
}
