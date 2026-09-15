import { useEffect, useRef } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { RefEntry } from '../api/types';
import { useRequest } from '../hooks/useRequest';
import { Loading, ErrorBanner } from './Status';

interface Props {
  client: WebspecClient;
  spec: string;
  anchor: string;
}

export function Section({ client, spec, anchor }: Props) {
  const state = useRequest<{ type: 'query'; result: import('../api/types').QueryResult }>(
    client,
    { type: 'query', target: `${spec}#${anchor}`, render: 'html' },
    [client, spec, anchor],
  );

  useEffect(() => {
    if (state.kind === 'ok') {
      const { result } = state.value;
      document.title = `${result.spec}#${result.anchor} · webspec-index`;
    } else {
      document.title = 'webspec-index';
    }
    return () => { document.title = 'webspec-index'; };
  }, [state]);

  if (state.kind === 'loading') return <div class="page"><Loading /></div>;
  if (state.kind === 'error') {
    return (
      <div class="page">
        <ErrorBanner code={state.code} message={state.message} spec={spec} anchor={anchor} />
      </div>
    );
  }

  const result = state.value.result;
  const nav = result.navigation;

  return (
    <div class="page content-column">
      <div class="section-meta">
        <span class={`badge badge-${result.type}`}>{result.type}</span>
        <a href={`#/${result.spec}`} class="mono">{result.spec}</a>
        <span class="mono" title={result.sha}>{result.sha.slice(0, 7)}</span>
        <a href={result.url} target="_blank" rel="noopener" style={{ fontSize: 'var(--text-xs)' }}>
          Open in spec ↗
        </a>
      </div>

      <h1 class="section-title">{result.title ?? result.anchor}</h1>
      <p class="mono" style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)', marginBottom: 'var(--space-2)' }}>
        {result.spec}#{result.anchor}
      </p>

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

      {nav.children.length > 0 && (
        <ul class="children-list" aria-label="Child sections">
          {nav.children.map((c) => (
            <li key={c.anchor}>
              <a href={`#/${result.spec}/${c.anchor}`}>{c.title ?? c.anchor}</a>
            </li>
          ))}
        </ul>
      )}

      {result.content_html ? (
        <ContentBlock html={result.content_html} />
      ) : result.content ? (
        <pre class="section-content">{result.content}</pre>
      ) : null}

      <RefSection heading="Outgoing references" refs={result.outgoing_refs} />
      <RefSection heading="Incoming references" refs={result.incoming_refs} />
    </div>
  );
}

interface ContentBlockProps {
  html: string;
}

function ContentBlock({ html }: ContentBlockProps) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    for (const a of el.querySelectorAll<HTMLAnchorElement>('a[href]')) {
      const href = a.getAttribute('href') ?? '';
      if (href.startsWith('#/') || href.startsWith('#')) continue;
      if (href.startsWith('http://') || href.startsWith('https://')) {
        a.setAttribute('target', '_blank');
        a.setAttribute('rel', 'noopener');
      }
    }
  }, [html]);

  return (
    <div
      class="section-content"
      ref={ref}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

interface RefSectionProps {
  heading: string;
  refs: RefEntry[];
}

function RefSection({ heading, refs }: RefSectionProps) {
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
                {ref.step_text && <span class="ref-step">{ref.step_text}</span>}
                {ref.step_path && !ref.step_text && <span class="ref-step">step {ref.step_path}</span>}
                {ref.kind && ref.kind !== 'prose' && <span class={`badge badge-${ref.kind}`}>{ref.kind}</span>}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}
