import { useEffect, useRef, useState } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { RefEntry } from '../api/types';
import { routeToHash } from '../router';
import { useRequest } from '../hooks/useRequest';
import { Loading, ErrorBanner } from './Status';
import { EffectsPanel } from './EffectsPanel';
import { RefsGraph } from './RefsGraph';
import { FlowView } from './FlowView';
import { remember } from '../trace/titles';
import { annotateSteps } from '../content/steps';
import { effectLabel } from '../effects/label';
import { SectionNumber } from './SectionNumber';
import type { StepEffects } from '../effects/inline';

interface Props {
  client: WebspecClient;
  spec: string;
  anchor: string;
  selectedStepPath?: number[];
}

const REFS_GRAPH_KEY = 'refsGraph';
const SECTION_VIEW_KEY = 'sectionView';
type SectionView = 'text' | 'flow';

function readGraphPref(): boolean {
  try {
    return localStorage.getItem(REFS_GRAPH_KEY) === 'true';
  } catch {
    return false;
  }
}

function writeGraphPref(v: boolean): void {
  try {
    localStorage.setItem(REFS_GRAPH_KEY, v ? 'true' : 'false');
  } catch {
    // ignore
  }
}

function readSectionView(): SectionView {
  try {
    return localStorage.getItem(SECTION_VIEW_KEY) === 'flow' ? 'flow' : 'text';
  } catch {
    return 'text';
  }
}

function writeSectionView(v: SectionView): void {
  try {
    localStorage.setItem(SECTION_VIEW_KEY, v);
  } catch {
    // ignore
  }
}

export function Section({ client, spec, anchor, selectedStepPath }: Props) {
  const [stepFx, setStepFx] = useState<StepEffects | undefined>(undefined);
  const [focusEffectId, setFocusEffectId] = useState<string | undefined>(undefined);
  const asideRef = useRef<HTMLElement>(null);
  const [stepCardTop, setStepCardTop] = useState(0);
  const stepKey = selectedStepPath?.join('.');
  const [graphOn, setGraphOn] = useState(readGraphPref);
  const [sectionView, setSectionView] = useState<SectionView>(readSectionView);

  // Place the step's details card level with the step in the text column.
  useEffect(() => {
    const aside = asideRef.current;
    if (!aside || !stepKey) return;
    function place() {
      const li = document.querySelector<HTMLLIElement>(`.section-main li[data-step-path="${stepKey}"]`);
      if (!li || !aside) return;
      const top = li.getBoundingClientRect().top - aside.getBoundingClientRect().top;
      setStepCardTop(Math.max(0, Math.round(top)));
    }
    place();
    window.addEventListener('resize', place);
    let ro: ResizeObserver | undefined;
    if (typeof ResizeObserver !== 'undefined') {
      ro = new ResizeObserver(place);
      ro.observe(aside);
    }
    return () => {
      window.removeEventListener('resize', place);
      ro?.disconnect();
    };
  }, [stepKey, stepFx, sectionView]);
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
    <div class="page section-layout">
    <div class="section-main">
      <div class="section-meta">
        <span class={`badge badge-${result.type}`}>{result.type}</span>
        <a href={`#/${result.spec}`} class="mono">{result.spec}</a>
        <span class="mono" title={result.sha}>{result.sha.slice(0, 7)}</span>
        <a href={result.url} target="_blank" rel="noopener" style={{ fontSize: 'var(--text-xs)' }}>
          Open in spec ↗
        </a>
      </div>

      <h1 class="section-title">
        <SectionNumber number={result.number} />
        {result.title ?? result.anchor}
      </h1>
      <p class="mono" style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)', marginBottom: 'var(--space-2)' }}>
        {result.spec}#{result.anchor}
      </p>

      <nav class="nav-strip" aria-label="Section navigation">
        {nav.parent && (
          <span class="nav-strip-item">
            <span class="nav-label">parent</span>
            <a href={`#/${result.spec}/${nav.parent.anchor}`}>
              <SectionNumber number={nav.parent.number} />
              {nav.parent.title ?? nav.parent.anchor}
            </a>
          </span>
        )}
        {nav.prev && (
          <span class="nav-strip-item">
            <span class="nav-label">← prev</span>
            <a href={`#/${result.spec}/${nav.prev.anchor}`}>
              <SectionNumber number={nav.prev.number} />
              {nav.prev.title ?? nav.prev.anchor}
            </a>
          </span>
        )}
        {nav.next && (
          <span class="nav-strip-item">
            <span class="nav-label">next →</span>
            <a href={`#/${result.spec}/${nav.next.anchor}`}>
              <SectionNumber number={nav.next.number} />
              {nav.next.title ?? nav.next.anchor}
            </a>
          </span>
        )}
      </nav>

      {nav.children.length > 0 && (
        <ul class="children-list" aria-label="Child sections">
          {nav.children.map((c) => (
            <li key={c.anchor}>
              <a href={`#/${result.spec}/${c.anchor}`}>
                <SectionNumber number={c.number} />
                {c.title ?? c.anchor}
              </a>
            </li>
          ))}
        </ul>
      )}

      {result.type === 'algorithm' && (
        <div class="section-view-toggle" role="group" aria-label="Content view">
          <button
            type="button"
            class="section-view-btn"
            aria-pressed={sectionView === 'text'}
            onClick={() => {
              setSectionView('text');
              writeSectionView('text');
            }}
          >
            Text
          </button>
          <button
            type="button"
            class="section-view-btn"
            aria-pressed={sectionView === 'flow'}
            onClick={() => {
              setSectionView('flow');
              writeSectionView('flow');
            }}
          >
            Flow
          </button>
        </div>
      )}

      {sectionView === 'flow' && result.type === 'algorithm' ? (
        <FlowView
          client={client}
          spec={result.spec}
          anchor={result.anchor}
          selectedId={selectedStepPath?.join('.')}
          onSelect={(id) =>
            handleStepSelect(selectedStepPath?.join('.') === id ? undefined : id.split('.').map(Number))
          }
        />
      ) : result.content_html ? (
        <ContentBlock
          html={result.content_html}
          selectedStepPath={selectedStepPath}
          onStepSelect={handleStepSelect}
          onEffectFocus={setFocusEffectId}
          stepEffects={stepFx}
        />
      ) : result.content ? (
        <pre class="section-content">{result.content}</pre>
      ) : null}

      <div class="refs-graph-toggle-row">
        <button
          type="button"
          class={`refs-graph-toggle${graphOn ? ' refs-graph-toggle--on' : ''}`}
          aria-pressed={graphOn}
          onClick={() => {
            const next = !graphOn;
            setGraphOn(next);
            writeGraphPref(next);
          }}
        >
          Graph
        </button>
      </div>
      {graphOn && (
        <RefsGraph client={client} spec={result.spec} anchor={result.anchor} />
      )}
      <RefSection heading="Outgoing references" refs={result.outgoing_refs} />
      <RefSection heading="Incoming references" refs={result.incoming_refs} />
    </div>

    <aside class="section-aside" aria-label="Details" ref={asideRef}>
      <div class="section-aside-summary" hidden={selectedStepPath !== undefined}>
        <EffectsPanel
          client={client}
          spec={result.spec}
          anchor={result.anchor}
          compact
          onStepEffects={setStepFx}
        />
      </div>
      {selectedStepPath && (
        <div class="section-step-card" style={{ top: `${stepCardTop}px` }}>
          <EffectsPanel
            client={client}
            spec={result.spec}
            anchor={result.anchor}
            selectedStepPath={selectedStepPath}
            focusEffectId={focusEffectId}
            onClearStep={() => handleStepSelect(undefined)}
          />
        </div>
      )}
    </aside>
    </div>
  );
}

interface ContentBlockProps {
  html: string;
  selectedStepPath?: number[];
  onStepSelect: (path: number[] | undefined) => void;
  onEffectFocus?: (effectId: string) => void;
  stepEffects?: StepEffects;
}

const INLINE_EFFECTS_CAP = 3;

/// Places a badge row under each step that has effects: the first few labels, then a
/// "+N" counter. Rebuilt from scratch whenever the attribution changes.
function renderInlineEffects(root: HTMLElement, stepEffects: StepEffects | undefined) {
  for (const old of root.querySelectorAll('.step-effects')) old.remove();
  if (!stepEffects) return;
  for (const [key, list] of stepEffects) {
    const li = root.querySelector<HTMLLIElement>(`li[data-step-path="${key}"]`);
    if (!li || list.length === 0) continue;
    const row = document.createElement('div');
    row.className = 'step-effects';
    row.setAttribute('role', 'note');
    for (const { effect, via } of list.slice(0, INLINE_EFFECTS_CAP)) {
      const badge = document.createElement('button');
      badge.type = 'button';
      badge.className = `step-effect step-effect-${via}`;
      badge.dataset.effectId = effect.id;
      badge.textContent = effectLabel(effect);
      badge.title = via === 'direct' ? 'Effect of this step' : 'Effect reached through a call in this step';
      row.appendChild(badge);
    }
    if (list.length > INLINE_EFFECTS_CAP) {
      const more = document.createElement('button');
      more.type = 'button';
      more.className = 'step-effect step-effect-more';
      more.textContent = `+${list.length - INLINE_EFFECTS_CAP} more`;
      row.appendChild(more);
    }
    const nested = li.querySelector(':scope > ol, :scope > ul');
    if (nested) li.insertBefore(row, nested);
    else li.appendChild(row);
  }
}

function ContentBlock({ html, selectedStepPath, onStepSelect, onEffectFocus, stepEffects }: ContentBlockProps) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (el) renderInlineEffects(el, stepEffects);
  }, [html, stepEffects]);

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
      const btn = (e.target as Element).closest<HTMLButtonElement>('.step-select-btn, .step-effect');
      if (!btn) return;
      const li = btn.closest<HTMLLIElement>('li[data-step-path]');
      if (!li || !li.dataset.stepPath) return;
      const pathStr = li.dataset.stepPath;
      const path = pathStr.split('.').map(Number);
      if (btn.classList.contains('step-effect')) {
        // A badge always selects its step and brings that effect into view.
        onStepSelect(path);
        if (btn.dataset.effectId && onEffectFocus) onEffectFocus(btn.dataset.effectId);
        return;
      }
      onStepSelect(
        selectedStepPath && selectedStepPath.join('.') === pathStr ? undefined : path,
      );
    }
    el.addEventListener('click', handleClick);
    return () => el.removeEventListener('click', handleClick);
  }, [html, selectedStepPath, onStepSelect, onEffectFocus]);

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
