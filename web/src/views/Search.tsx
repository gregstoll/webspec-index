import { useEffect } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { SpecEntry } from '../api/types';
import { useRequest } from '../hooks/useRequest';
import { Loading, ErrorBanner } from './Status';
import { routeToHash } from '../router';

interface Props {
  client: WebspecClient;
  query: string;
  spec?: string;
}

// The FTS snippet function uses <mark>...</mark> markers (trusted, from SQLite FTS).
// Render via dangerouslySetInnerHTML so <mark> tags highlight matches.
function Snippet({ text }: { text: string }) {
  return (
    <div
      class="search-result-snippet"
      dangerouslySetInnerHTML={{ __html: text }}
    />
  );
}

export function Search({ client, query, spec }: Props) {
  const searchState = useRequest<{ type: 'search'; result: import('../api/types').SearchResult }>(
    client,
    query ? (spec ? { type: 'search', query, spec } : { type: 'search', query }) : null,
    [client, query, spec],
  );

  const specsState = useRequest<{ type: 'specs'; result: { specs: SpecEntry[] } }>(
    client,
    { type: 'specs' },
    [client],
  );

  useEffect(() => {
    document.title = `search: ${query} · webspec-index`;
    return () => { document.title = 'webspec-index'; };
  }, [query]);

  const specNames = specsState.kind === 'ok'
    ? specsState.value.result.specs.map((s) => s.name)
    : [];

  function onSpecChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value;
    const hash = routeToHash({ kind: 'search', q: query, spec: value || undefined });
    window.location.hash = hash;
  }

  return (
    <div class="page">
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--space-4)', flexWrap: 'wrap', marginBottom: 'var(--space-6)' }}>
        <h1 class="section-title" style={{ marginBottom: 0 }}>
          Search: <em style={{ fontStyle: 'normal', fontFamily: 'var(--font-mono)' }}>{query}</em>
          {spec && <span style={{ color: 'var(--color-text-muted)', fontWeight: 400, fontSize: 'var(--text-xl)' }}> in {spec}</span>}
        </h1>
        <label style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
          Filter:{' '}
          <select
            value={spec ?? ''}
            onChange={onSpecChange}
            style={{ fontFamily: 'var(--font-mono)', fontSize: 'var(--text-sm)', padding: '2px 4px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--color-border)', background: 'var(--color-bg)', color: 'var(--color-text)' }}
          >
            <option value="">All specs</option>
            {specNames.map((name) => (
              <option key={name} value={name}>{name}</option>
            ))}
          </select>
        </label>
      </div>

      {searchState.kind === 'loading' && <Loading />}
      {searchState.kind === 'error' && (
        <ErrorBanner code={searchState.code} message={searchState.message} />
      )}
      {searchState.kind === 'ok' && (
        <>
          {searchState.value.result.results.length === 0 && (
            <p style={{ color: 'var(--color-text-muted)', marginTop: 'var(--space-6)' }}>No results.</p>
          )}
          <div class="search-results">
            {searchState.value.result.results.map((r) => (
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
                {r.snippet && <Snippet text={r.snippet} />}
              </a>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
