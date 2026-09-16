import { useMemo } from 'preact/hooks';
import type { TraceEntry } from '../trace/store';
import { getTitleCache } from '../trace/titles';
import { traceGraph } from '../diagram/traceGraph';
import { Diagram } from '../diagram/Diagram';

interface TraceDiagramProps {
  entries: readonly TraceEntry[];
  rankdir?: 'TB' | 'LR';
}

export function TraceDiagram({ entries, rankdir = 'LR' }: TraceDiagramProps) {
  const graph = useMemo(() => traceGraph(entries, getTitleCache()), [entries]);

  return (
    <Diagram
      graph={graph}
      rankdir={rankdir}
      ariaLabel="Trace diagram"
    />
  );
}
