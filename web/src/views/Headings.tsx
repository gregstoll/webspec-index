import { useState, useEffect } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { ListEntry } from '../api/types';

interface Props {
  client: WebspecClient;
  spec: string;
}

export function Headings({ client, spec }: Props) {
  const [entries, setEntries] = useState<ListEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    setError(null);
    client.request({ type: 'list', spec }).then((resp) => {
      if (resp.type === 'list') {
        setEntries(resp.result.entries);
      } else if (resp.type === 'error') {
        setError(resp.message);
      }
    }).catch((e: unknown) => setError(String(e))).finally(() => setLoading(false));
  }, [client, spec]);

  if (loading) return <div class="page loading">Loading…</div>;
  if (error) return (
    <div class="page">
      <div class="error-banner">{error}</div>
    </div>
  );

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
