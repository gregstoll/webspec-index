import type { WebspecClient } from '../api/client';
import type { FlowResult } from '../api/types';
import { useRequest } from '../hooks/useRequest';
import { Diagram } from '../diagram/Diagram';
import { buildFlowGraph } from '../diagram/flowGraph';
import { Loading, ErrorBanner } from './Status';

interface Props {
  client: WebspecClient;
  spec: string;
  anchor: string;
  selectedId?: string;
  onSelect?: (id: string) => void;
}

export function FlowView({ client, spec, anchor, selectedId, onSelect }: Props) {
  const state = useRequest<{ type: 'flow'; result: FlowResult }>(
    client,
    { type: 'flow', target: `${spec}#${anchor}` },
    [client, spec, anchor],
  );

  if (state.kind === 'loading') return <Loading />;
  if (state.kind === 'error') {
    return <ErrorBanner code={state.code} message={state.message} spec={spec} anchor={anchor} />;
  }

  const { result } = state.value;
  const graph = buildFlowGraph(result);

  return (
    <div class="flow-view">
      <Diagram
        graph={graph}
        rankdir="TB"
        selectedId={selectedId}
        onSelect={onSelect}
        ariaLabel={`Algorithm flowchart for ${spec}#${anchor}`}
      />
      {result.issues.length > 0 && (
        <details class="flow-issues">
          <summary>{result.issues.length} steps not understood</summary>
          <ul>
            {result.issues.map((issue, i) => (
              <li key={i}><span class="mono">{issue.step}</span>: {issue.message}</li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
