import { useState, useEffect } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { SpecEntry } from '../api/types';
import { navigateForQuery } from '../dispatch';
import { useRequest } from '../hooks/useRequest';
import { Loading } from './Status';

interface Props {
  client: WebspecClient;
}

const PROVIDER_LABELS: Record<string, string> = {
  whatwg: 'WHATWG',
  w3c: 'W3C',
  csswg: 'W3C',
  tc39: 'TC39',
};

function providerGroup(provider: string): string {
  return PROVIDER_LABELS[provider] ?? 'Other';
}

export function Landing({ client }: Props) {
  const [query, setQuery] = useState('');

  const state = useRequest<{ type: 'specs'; result: { specs: SpecEntry[] } }>(
    client,
    { type: 'specs' },
    [client],
  );

  useEffect(() => {
    document.title = 'webspec-index';
  }, []);

  function handleSubmit(e: Event) {
    e.preventDefault();
    navigateForQuery(query);
  }

  const specs = state.kind === 'ok' ? state.value.result.specs : [];

  const groups = new Map<string, SpecEntry[]>();
  for (const spec of specs) {
    const group = providerGroup(spec.provider);
    const arr = groups.get(group) ?? [];
    arr.push(spec);
    groups.set(group, arr);
  }
  const ORDER = ['WHATWG', 'W3C', 'TC39', 'Other'];
  const sortedGroups = [...groups.entries()].sort(
    ([a], [b]) => ORDER.indexOf(a) - ORDER.indexOf(b),
  );

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
        {state.kind === 'loading' ? (
          <Loading />
        ) : (
          <div>
            {sortedGroups.map(([group, groupSpecs]) => (
              <div key={group} style={{ marginBottom: 'var(--space-6)' }}>
                <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, color: 'var(--color-text-secondary)', marginBottom: 'var(--space-2)' }}>
                  {group}
                </h3>
                <div class="spec-grid">
                  {groupSpecs.map((s) => (
                    <a key={s.name} href={`#/${s.name}`} class="spec-card">
                      <span class="spec-card-name">{s.name}</span>
                      <span class="spec-card-provider" style={{ fontFamily: 'var(--font-mono)', fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)' }}>
                        {s.sha.slice(0, 7)} · {s.commit_date}
                      </span>
                    </a>
                  ))}
                </div>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
