import { useEffect, useRef } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { RefEntry } from '../api/types';
import { routeToHash } from '../router';
import { useRequest } from '../hooks/useRequest';
import { Loading, ErrorBanner } from './Status';
import { EffectsPanel } from './EffectsPanel';
import { remember } from '../trace/titles';
import { annotateSteps } from '../content/steps';

interface Props {
  client: WebspecClient;
  spec: string;
  anchor: string;
  selectedStepPath?: number[];
}

export function Section({ client, spec, anchor, selectedStepPath }: Props) {
  const state = useRequest<{ type: 'query'; result: import('../api/types').QueryResult }>(
    client,
    { type: 'query', target: `${spec}#${anchor}`, render: 'html' },
    [client, spec, anchor],
  );

  useEffect(() => {
    if (state.kind === 'ok') {
      const { result } = state.value;
      document.title = `${result.spec}#${result.anchor} · webspec-index`;
      remember(result);
    } else {
      document.title = 'webspec-index';
    }
    return () => { document.title = 'webspec-index'; };
  }, [state]);

  function handleStepSelect(path: number[] | undefined) {
    location.hash = path
      ? routeToHash({ kind: 'section', spec, anchor, step: path })
      : routeToHash({ kind: 'section', spec, anchor });
  }

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
        <ContentBlock
          html={result.content_html}
          selectedStepPath={selectedStepPath}
          onStepSelect={handleStepSelect}
        />
      ) : result.content ? (
        <pre class="section-content">{result.content}</pre>
      ) : null}

      <RefSection heading="Outgoing references" refs={result.outgoing_refs} />
      <RefSection heading="Incoming references" refs={result.incoming_refs} />

      <EffectsPanel
        client={client}
        spec={result.spec}
        anchor={result.anchor}
        selectedStepPath={selectedStepPath}
        onClearStep={() => handleStepSelect(undefined)}
      />
    </div>
  );
}

interface ContentBlockProps {
  html: string;
  selectedStepPath?: number[];
  onStepSelect: (path: number[] | undefined) => void;
}

function ContentBlock({ html, selectedStepPath, onStepSelect }: ContentBlockProps) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    for (const a of el.querySelectorAll<HTMLAnchorElement>('a[href]')) {
      const href = a.getAttribute('href') ?? '';
      if (href.startsWith('#')) continue;
      if (/^\s*javascript:/i.test(href)) {
        a.removeAttribute('href');
        continue;
      }
      if (href.startsWith('http://') || href.startsWith('https://')) {
        a.setAttribute('target', '_blank');
        a.setAttribute('rel', 'noopener');
      }
    }
    annotateSteps(el);
  }, [html]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    function handleClick(e: MouseEvent) {
      const btn = (e.target as Element).closest<HTMLButtonElement>('.step-select-btn');
      if (!btn) return;
      const li = btn.closest<HTMLLIElement>('li[data-step-path]');
      if (!li || !li.dataset.stepPath) return;
      const pathStr = li.dataset.stepPath;
      const path = pathStr.split('.').map(Number);
      onStepSelect(
        selectedStepPath && selectedStepPath.join('.') === pathStr ? undefined : path,
      );
    }
    el.addEventListener('click', handleClick);
    return () => el.removeEventListener('click', handleClick);
  }, [html, selectedStepPath, onStepSelect]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    for (const li of el.querySelectorAll<HTMLLIElement>('li[aria-current]')) {
      li.removeAttribute('aria-current');
      li.classList.remove('step-selected');
    }
    if (selectedStepPath) {
      const key = selectedStepPath.join('.');
      const li = el.querySelector<HTMLLIElement>(`li[data-step-path="${key}"]`);
      if (li) {
        li.setAttribute('aria-current', 'true');
        li.classList.add('step-selected');
      }
    }
  }, [selectedStepPath]);

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
    <details class="refs-section" open={refs.length <= 8}>
      <summary>
        <h3>
          {heading}{' '}
          <span style={{ color: 'var(--color-text-muted)', fontWeight: 400, fontSize: 'var(--text-base)' }}>
            ({refs.length})
          </span>
        </h3>
      </summary>
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
    </details>
  );
}
