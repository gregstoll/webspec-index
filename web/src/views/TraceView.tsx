import { useState, useEffect, useRef } from 'preact/hooks';
import { decodeTrace } from '../trace/share';
import { traceStore } from '../trace/store';
import type { TraceEntry } from '../trace/store';
import { getTitleCache } from '../trace/titles';
import { routeToHash } from '../router';
import { ErrorBanner } from './Status';
import '../styles/trace.css';

interface TraceViewProps {
  payload: string;
}

type DecodeState =
  | { kind: 'loading' }
  | { kind: 'ok'; entries: Omit<TraceEntry, 'id'>[] }
  | { kind: 'error'; message: string };

function stepLabel(step_path?: number[]): string {
  if (!step_path || step_path.length === 0) return '';
  return `step ${step_path.join('.')}`;
}

function entryTitle(entry: Omit<TraceEntry, 'id'>): string {
  const key = `${entry.spec}#${entry.anchor}`;
  const cached = getTitleCache().get(key);
  return cached ? cached.title : key;
}

function entryHref(entry: Omit<TraceEntry, 'id'>): string {
  return routeToHash({
    kind: 'section',
    spec: entry.spec,
    anchor: entry.anchor,
    ...(entry.step_path && entry.step_path.length > 0 ? { step: entry.step_path } : {}),
  });
}

export function TraceView({ payload }: TraceViewProps) {
  const [state, setState] = useState<DecodeState>({ kind: 'loading' });
  const [importFeedback, setImportFeedback] = useState('');
  const [imported, setImported] = useState(false);
  const importedRef = useRef(false);

  useEffect(() => {
    decodeTrace(payload)
      .then((entries) => setState({ kind: 'ok', entries }))
      .catch((err: unknown) =>
        setState({ kind: 'error', message: err instanceof Error ? err.message : String(err) }),
      );
  }, [payload]);

  if (state.kind === 'loading') {
    return <div class="page loading">Loading trace…</div>;
  }

  if (state.kind === 'error') {
    return (
      <div class="page">
        <ErrorBanner code="decode_error" message={state.message} />
      </div>
    );
  }

  const { entries } = state;

  function importAll() {
    if (importedRef.current) return;
    importedRef.current = true;
    setImported(true);
    for (const entry of entries) traceStore.add(entry);
    if (entries.length > 0) location.hash = entryHref(entries[0]);
    setImportFeedback('Imported!');
    setTimeout(() => setImportFeedback(''), 2000);
  }

  function openFirst() {
    if (entries.length > 0) location.hash = entryHref(entries[0]);
  }

  return (
    <div class="page trace-view">
      <h1 class="trace-view-heading">
        Shared trace — {entries.length} step{entries.length !== 1 ? 's' : ''}
      </h1>

      <ol class="trace-view-list">
        {entries.map((e, i) => {
          const href = entryHref(e);
          const title = entryTitle(e);
          const step = stepLabel(e.step_path);
          return (
            <li class="trace-view-entry" key={i}>
              <div class="trace-view-entry-body">
                <a class="trace-view-link" href={href}>
                  {title}
                  {step && <span class="trace-view-step"> {step}</span>}
                </a>
                {e.note && <p class="trace-view-note">{e.note}</p>}
              </div>
            </li>
          );
        })}
      </ol>

      <div class="trace-view-actions">
        <button class="trace-action-btn primary" onClick={importAll} disabled={imported}>
          Import into my recorder
        </button>
        {importFeedback && <span class="trace-copy-feedback">{importFeedback}</span>}
        {entries.length > 0 && (
          <button class="trace-action-btn" onClick={openFirst}>
            Open first entry
          </button>
        )}
      </div>
    </div>
  );
}
