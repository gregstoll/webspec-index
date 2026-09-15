import { useState, useEffect } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { QueryResult, RefEntry } from '../api/types';
import { NotFound } from './NotFound';

interface Props {
  client: WebspecClient;
  spec: string;
  anchor: string;
}

export function Section({ client, spec, anchor }: Props) {
  const [result, setResult] = useState<QueryResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    setError(null);
    setResult(null);
    client.request({ type: 'query', target: `${spec}#${anchor}`, render: 'html' }).then((resp) => {
      if (resp.type === 'query') {
        setResult(resp.result);
      } else if (resp.type === 'error') {
        setError(resp.message);
      }
    }).catch((e: unknown) => setError(String(e))).finally(() => setLoading(false));
  }, [client, spec, anchor]);

  if (loading) return <div class="page loading">Loading…</div>;
  if (error) return <NotFound message={error} />;
  if (!result) return <NotFound />;

  const nav = result.navigation;

  return (
    <div class="page content-column">
      <div class="section-meta">
        <span class={`badge badge-${result.type}`}>{result.type}</span>
        <a href={`#/${result.spec}`} class="mono">{result.spec}</a>
        <span class="mono" title="Snapshot SHA">{result.sha.slice(0, 8)}</span>
        <a href={result.url} target="_blank" rel="noreferrer" style={{ fontSize: 'var(--text-xs)' }}>
          live spec ↗
        </a>
      </div>

      <h1 class="section-title">{result.title ?? result.anchor}</h1>
      <p class="mono" style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)', marginBottom: 'var(--space-2)' }}>
        {result.spec}#{result.anchor}
      </p>

      {/* Navigation */}
      <nav class="nav-strip" aria-label="Section navigation">
        {nav.parent && (
          <span class="nav-strip-item">
            <span class="nav-label">parent</span>
            <a href={`#/${result.spec}/${nav.parent.anchor}`}>{nav.parent.title ?? nav.parent.anchor}</a>
          </span>
        )}
        {nav.prev && (
          <span class="nav-strip-item">
            <span class="nav-label">← prev</span>
            <a href={`#/${result.spec}/${nav.prev.anchor}`}>{nav.prev.title ?? nav.prev.anchor}</a>
          </span>
        )}
        {nav.next && (
          <span class="nav-strip-item">
            <span class="nav-label">next →</span>
            <a href={`#/${result.spec}/${nav.next.anchor}`}>{nav.next.title ?? nav.next.anchor}</a>
          </span>
        )}
      </nav>

      {/* Children */}
      {nav.children.length > 0 && (
        <ul class="children-list" aria-label="Child sections">
          {nav.children.map((c) => (
            <li key={c.anchor}>
              <a href={`#/${result.spec}/${c.anchor}`}>{c.title ?? c.anchor}</a>
            </li>
          ))}
        </ul>
      )}

      {/* Content */}
      {result.content_html ? (
        <div class="section-content" dangerouslySetInnerHTML={{ __html: result.content_html }} />
      ) : result.content ? (
        <pre class="section-content">{result.content}</pre>
      ) : null}

      {/* References */}
      <RefSection heading="Outgoing references" refs={result.outgoing_refs} specName={result.spec} />
      <RefSection heading="Incoming references" refs={result.incoming_refs} specName={result.spec} />
    </div>
  );
}

interface RefSectionProps {
  heading: string;
  refs: RefEntry[];
  specName: string;
}

function RefSection({ heading, refs, specName: _specName }: RefSectionProps) {
  if (refs.length === 0) return null;

  const bySpec = new Map<string, RefEntry[]>();
  for (const ref of refs) {
    const group = bySpec.get(ref.spec) ?? [];
    group.push(ref);
    bySpec.set(ref.spec, group);
  }

  return (
    <div class="refs-section">
      <h3>
        {heading}{' '}
        <span style={{ color: 'var(--color-text-muted)', fontWeight: 400, fontSize: 'var(--text-base)' }}>
          ({refs.length})
        </span>
      </h3>
      {Array.from(bySpec.entries()).map(([spec, group]) => (
        <div key={spec} class="refs-group">
          <div class="refs-group-header">{spec}</div>
          <ul class="refs-list">
            {group.map((ref, i) => (
              <li key={`${ref.anchor}-${i}`} class="ref-entry">
                <a href={`#/${ref.spec}/${ref.anchor}`} class="mono">{ref.anchor}</a>
                {ref.step_path && <span class="ref-step">step {ref.step_path}</span>}
                {ref.kind && ref.kind !== 'prose' && <span class={`badge badge-${ref.kind}`}>{ref.kind}</span>}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}
