import { useState, useEffect } from 'preact/hooks';
import { parseRoute, routeToHash } from './router';
import { WorkerClient } from './api/client';
import type { WebspecClient } from './api/client';
import { Landing } from './views/Landing';
import { Section } from './views/Section';
import { Search } from './views/Search';
import { Headings } from './views/Headings';
import { NotFound } from './views/NotFound';
import { navigateForQuery } from './dispatch';

// The worker answers from MockClient via loadBackend(). Swap loadBackend() in
// worker.ts to use the wasm client once webspec-index-wasm is built.
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
            <SearchBox />
          </div>
          <button class="theme-toggle" onClick={cycleTheme} title="Toggle theme" aria-label="Toggle colour theme">
            {themeLabel}
          </button>
        </div>
      </header>

      {view}

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

function SearchBox() {
  const [q, setQ] = useState('');

  function handleSubmit(e: Event) {
    e.preventDefault();
    navigateForQuery(q);
    setQ('');
  }

  return (
    <form onSubmit={handleSubmit}>
      <input
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
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    client.request({ type: 'query', target: url }).then((resp) => {
      if (resp.type === 'error') {
        setError(resp.message);
        return;
      }
      if (resp.type === 'query') {
        window.location.hash = `#/${resp.result.spec}/${resp.result.anchor}`;
      }
    }).catch((e: unknown) => setError(String(e)));
  }, [client, url]);

  if (error) return <NotFound message={error} />;
  return <div class="page loading">Resolving…</div>;
}
