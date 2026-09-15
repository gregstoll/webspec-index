import { useEffect } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import { useRequest } from '../hooks/useRequest';
import { Loading, ErrorBanner } from './Status';

interface Props {
  client: WebspecClient;
  spec: string;
}

export function Headings({ client, spec }: Props) {
  const state = useRequest<{ type: 'list'; result: { spec: string; entries: import('../api/types').ListEntry[] } }>(
    client,
    { type: 'list', spec },
    [client, spec],
  );

  useEffect(() => {
    document.title = `${spec} · webspec-index`;
    return () => { document.title = 'webspec-index'; };
  }, [spec]);

  if (state.kind === 'loading') return <div class="page"><Loading /></div>;
  if (state.kind === 'error') {
    return (
      <div class="page">
        <ErrorBanner code={state.code} message={state.message} spec={spec} />
      </div>
    );
  }

  const { entries } = state.value.result;

  return (
    <div class="page">
      <h1 class="section-title">
        <span class="mono">{spec}</span> — headings
      </h1>
      <ul class="headings-list" aria-label={`Headings in ${spec}`}>
        {entries.map((entry) => (
          <li key={entry.anchor} class="headings-entry" style={{ paddingLeft: `${(entry.depth - 2) * 1.25}rem` }}>
            <a href={`#/${spec}/${entry.anchor}`} class="mono" style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)' }}>
              {entry.anchor}
            </a>
            {entry.title && (
              <a href={`#/${spec}/${entry.anchor}`} style={{ fontWeight: entry.depth <= 2 ? 600 : 400 }}>
                {entry.title}
              </a>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
