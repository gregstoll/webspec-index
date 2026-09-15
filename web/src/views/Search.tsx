import { useState, useEffect } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { SearchEntry } from '../api/types';

interface Props {
  client: WebspecClient;
  query: string;
  spec?: string;
}

export function Search({ client, query, spec }: Props) {
  const [results, setResults] = useState<SearchEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!query) {
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    const req = spec
      ? { type: 'search' as const, query, spec }
      : { type: 'search' as const, query };
    client.request(req).then((resp) => {
      if (resp.type === 'search') {
        setResults(resp.result.results);
      } else if (resp.type === 'error') {
        setError(resp.message);
      }
    }).catch((e: unknown) => setError(String(e))).finally(() => setLoading(false));
  }, [client, query, spec]);

  if (loading) return <div class="page loading">Searching…</div>;

  return (
    <div class="page">
      <h1 class="section-title">
        Search: <em style={{ fontStyle: 'normal', fontFamily: 'var(--font-mono)' }}>{query}</em>
        {spec && <span style={{ color: 'var(--color-text-muted)', fontWeight: 400, fontSize: 'var(--text-xl)' }}> in {spec}</span>}
      </h1>

      {error && <div class="error-banner">{error}</div>}

      {results.length === 0 && !error && (
        <p style={{ color: 'var(--color-text-muted)', marginTop: 'var(--space-6)' }}>No results.</p>
      )}

      <div class="search-results">
        {results.map((r) => (
          <a
            key={`${r.spec}#${r.anchor}`}
            href={`#/${r.spec}/${r.anchor}`}
            class="search-result-card"
            style={{ textDecoration: 'none' }}
          >
            <div class="search-result-title">
              <span class={`badge badge-${r.type}`}>{r.type}</span>{' '}
              {r.title ?? r.anchor}{' '}
              <span style={{ color: 'var(--color-text-muted)', fontFamily: 'var(--font-mono)', fontSize: 'var(--text-xs)' }}>
                {r.spec}#{r.anchor}
              </span>
            </div>
            {r.snippet && (
              <div
                class="search-result-snippet"
                dangerouslySetInnerHTML={{ __html: r.snippet }}
              />
            )}
          </a>
        ))}
      </div>
    </div>
  );
}
