import { useState, useEffect, useRef } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { GraphResult } from '../api/types';
import { Diagram } from '../diagram/Diagram';
import type { DiagramGraph } from '../diagram/model';
import { mergeGraphs } from '../diagram/model';
import { buildRefsGraph } from '../diagram/refsGraph';

interface Props {
  client: WebspecClient;
  spec: string;
  anchor: string;
}

export function RefsGraph({ client, spec, anchor }: Props) {
  const rootId = `${spec}#${anchor}`;
  const [graph, setGraph] = useState<DiagramGraph | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const expandedRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    let cancelled = false;
    setGraph(null);
    setError(null);
    setLoading(true);
    expandedRef.current = new Set();

    client
      .request({
        type: 'graph',
        target: `${spec}#${anchor}`,
        direction: 'both',
        max_depth: 1,
        max_nodes: 60,
      })
      .then((res) => {
        if (cancelled) return;
        if (res.type === 'error') {
          setError(res.message);
          setLoading(false);
          return;
        }
        if (res.type !== 'graph') return;
        const { graph: built } = buildRefsGraph(res.result as GraphResult);
        setGraph(built);
        setLoading(false);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : String(err));
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [client, spec, anchor]);

  function handleExpand(id: string) {
    if (expandedRef.current.has(id)) return;
    expandedRef.current.add(id);

    let cancelled = false;
    client
      .request({
        type: 'graph',
        target: id,
        direction: 'both',
        max_depth: 1,
        max_nodes: 60,
      })
      .then((res) => {
        if (cancelled) return;
        if (res.type !== 'graph') return;
        const { graph: built } = buildRefsGraph(res.result as GraphResult);
        setGraph((prev) => (prev != null ? mergeGraphs(prev, built) : built));
      })
      .catch(() => { /* ignore expand errors silently */ });

    return () => { cancelled = true; };
  }

  if (loading) {
    return <div class="refs-graph-loading" aria-busy="true">Loading graph…</div>;
  }

  if (error != null) {
    return <div class="refs-graph-error">{error}</div>;
  }

  if (graph === null) {
    return null;
  }

  return (
    <div class="refs-graph">
      <Diagram
        graph={graph}
        selectedId={rootId}
        onExpand={handleExpand}
        ariaLabel={`Reference graph for ${spec}#${anchor}`}
      />
    </div>
  );
}
