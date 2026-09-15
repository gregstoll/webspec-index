import { useState, useEffect } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { SpecEntry } from '../api/types';
import { navigateForQuery } from '../dispatch';

interface Props {
  client: WebspecClient;
}

export function Landing({ client }: Props) {
  const [query, setQuery] = useState('');
  const [specs, setSpecs] = useState<SpecEntry[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    client.request({ type: 'specs' }).then((resp) => {
      if (resp.type === 'specs') setSpecs(resp.result.specs);
    }).finally(() => setLoading(false));
  }, [client]);

  function handleSubmit(e: Event) {
    e.preventDefault();
    navigateForQuery(query);
  }

  return (
    <div class="page">
      <h1 class="section-title">webspec-index</h1>
      <p style={{ color: 'var(--color-text-secondary)', marginTop: 'var(--space-2)' }}>
        Browse WHATWG, W3C, and TC39 specifications. Enter a section name, <code>SPEC#anchor</code>, or a full spec URL.
      </p>

      <form class="landing-search-form" onSubmit={handleSubmit}>
        <input
          class="search-input"
          type="search"
          placeholder="navigate, HTML#navigate, https://html.spec.whatwg.org/#navigate …"
          value={query}
          onInput={(e) => setQuery((e.target as HTMLInputElement).value)}
          aria-label="Search specifications"
          autofocus
        />
        <button class="btn" type="submit">Go</button>
      </form>

      <section aria-label="Indexed specifications">
        <h2 style={{ fontSize: 'var(--text-lg)', fontWeight: 600, marginBottom: 'var(--space-2)' }}>
          Indexed specifications
        </h2>
        {loading ? (
          <div class="loading">Loading…</div>
        ) : (
          <div class="spec-grid">
            {specs.map((s) => (
              <a key={s.name} href={`#/${s.name}`} class="spec-card">
                <span class="spec-card-name">{s.name}</span>
                <span class="spec-card-provider">{s.provider}</span>
              </a>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
