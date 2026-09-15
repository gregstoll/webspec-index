import { useState, useEffect, useRef } from 'preact/hooks';
import { parseRoute, routeToHash } from './router';
import { WorkerClient } from './api/client';
import type { WebspecClient } from './api/client';
import { Landing } from './views/Landing';
import { Section } from './views/Section';
import { Search } from './views/Search';
import { Headings } from './views/Headings';
import { NotFound } from './views/NotFound';
import { ErrorBanner } from './views/Status';
import { TracePanel, TracePanelToggle } from './views/TracePanel';
import { useRequest } from './hooks/useRequest';
import { navigateForQuery } from './dispatch';
import { traceStore } from './trace/store';

const worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
const client: WebspecClient = new WorkerClient(worker);

type Theme = 'light' | 'dark' | 'system';

function resolvedTheme(theme: Theme): 'light' | 'dark' {
  if (theme !== 'system') return theme;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function App() {
  const [hash, setHash] = useState(() => window.location.hash || '#/');
  const [theme, setTheme] = useState<Theme>(() => {
    const stored = localStorage.getItem('theme');
    return stored === 'light' || stored === 'dark' ? stored : 'system';
  });
  const [traceOpen, setTraceOpen] = useState(false);
  const [traceCount, setTraceCount] = useState(() => traceStore.entries.length);
  const searchInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const resolved = resolvedTheme(theme);
    document.documentElement.setAttribute('data-theme', resolved);
    if (theme === 'system') {
      localStorage.removeItem('theme');
    } else {
      localStorage.setItem('theme', theme);
    }
  }, [theme]);

  useEffect(() => {
    const onHashChange = () => setHash(window.location.hash || '#/');
    window.addEventListener('hashchange', onHashChange);
    return () => window.removeEventListener('hashchange', onHashChange);
  }, []);

  useEffect(() => {
    return traceStore.subscribe((entries) => setTraceCount(entries.length));
  }, []);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const target = e.target as HTMLElement;
      if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable) return;
      if (e.key === '/') {
        e.preventDefault();
        searchInputRef.current?.focus();
        return;
      }
      if (e.key === 't') {
        const r = parseRoute(window.location.hash || '#/');
        if (r.kind === 'section') {
          traceStore.add({ spec: r.spec, anchor: r.anchor });
          setTraceOpen(true);
        }
      }
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  const route = parseRoute(hash);

  let view;
  switch (route.kind) {
    case 'landing':
      view = <Landing client={client} />;
      break;
    case 'section':
      view = <Section client={client} spec={route.spec} anchor={route.anchor} />;
      break;
    case 'search':
      view = <Search client={client} query={route.q} spec={route.spec} />;
      break;
    case 'headings':
      view = <Headings client={client} spec={route.spec} />;
      break;
    case 'resolve':
      view = <ResolveView client={client} url={route.url} />;
      break;
    case 'trace':
      view = <div class="page placeholder">Trace view — not yet implemented.</div>;
      break;
    default:
      view = <NotFound />;
  }

  function cycleTheme() {
    setTheme((t) => (t === 'system' ? 'light' : t === 'light' ? 'dark' : 'system'));
  }

  const themeLabel = theme === 'system' ? 'auto' : theme;

  return (
    <div class="app">
      <header class="app-header">
        <div class="app-header-inner">
          <a href={routeToHash({ kind: 'landing' })} class="app-header-logo">
            webspec-index
          </a>
          <div class="app-header-search">
            <SearchBox inputRef={searchInputRef} />
          </div>
          <TracePanelToggle count={traceCount} onClick={() => setTraceOpen((o) => !o)} />
          <button class="theme-toggle" onClick={cycleTheme} title="Toggle theme" aria-label="Toggle colour theme">
            {themeLabel}
          </button>
        </div>
      </header>

      {view}

      {traceOpen && <TracePanel route={route} onClose={() => setTraceOpen(false)} />}

      <footer class="app-footer">
        Spec content:{' '}
        <a href="https://whatwg.org/policies#ipr" target="_blank" rel="noreferrer">WHATWG CC BY 4.0</a>
        {' · '}
        <a href="https://www.w3.org/Consortium/Legal/2015/doc-license" target="_blank" rel="noreferrer">W3C Document License</a>
        {' · '}
        <a href="https://www.ecma-international.org/ecma-262/" target="_blank" rel="noreferrer">Ecma</a>
        {' · '}
        <a href="https://github.com/jnjaeschke/webspec-index" target="_blank" rel="noreferrer">Source</a>
      </footer>
    </div>
  );
}

interface SearchBoxProps {
  inputRef: import('preact').RefObject<HTMLInputElement>;
}

function SearchBox({ inputRef }: SearchBoxProps) {
  const [q, setQ] = useState('');

  function handleSubmit(e: Event) {
    e.preventDefault();
    navigateForQuery(q);
    setQ('');
  }

  return (
    <form onSubmit={handleSubmit}>
      <input
        ref={inputRef}
        class="search-input"
        type="search"
        placeholder="SPEC#anchor or search…"
        value={q}
        onInput={(e) => setQ((e.target as HTMLInputElement).value)}
        aria-label="Quick search"
      />
    </form>
  );
}

interface ResolveViewProps {
  client: WebspecClient;
  url: string;
}

function ResolveView({ client, url }: ResolveViewProps) {
  const state = useRequest<{ type: 'query'; result: import('./api/types').QueryResult }>(
    client,
    { type: 'query', target: url },
    [client, url],
  );

  useEffect(() => {
    if (state.kind === 'ok') {
      const { spec, anchor } = state.value.result;
      window.location.replace(`#/${spec}/${anchor}`);
    }
  }, [state]);

  if (state.kind === 'loading') return <div class="page loading">Resolving…</div>;
  if (state.kind === 'error') {
    return (
      <div class="page">
        <ErrorBanner code={state.code} message={state.message} />
      </div>
    );
  }
  return null;
}
