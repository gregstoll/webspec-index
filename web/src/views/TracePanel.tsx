import { useState, useEffect, useRef } from 'preact/hooks';
import { traceStore } from '../trace/store';
import type { TraceEntry } from '../trace/store';
import { getTitleCache } from '../trace/titles';
import { traceToMarkdown } from '../trace/markdown';
import { encodeTrace } from '../trace/share';
import type { Route } from '../router';
import '../styles/trace.css';

interface TracePanelProps {
  route: Route;
  selectedStepPath?: number[];
  onClose: () => void;
}

export function TracePanelToggle({ count, onClick }: { count: number; onClick: () => void }) {
  return (
    <button class="trace-toggle" onClick={onClick} title="Toggle trace recorder (t)" aria-label="Toggle trace recorder">
      trace
      {count > 0 && <span class="trace-toggle-count">{count}</span>}
    </button>
  );
}

export function TracePanel({ route, selectedStepPath, onClose }: TracePanelProps) {
  const [entries, setEntries] = useState<readonly TraceEntry[]>(() => traceStore.entries);
  const [copyFeedback, setCopyFeedback] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    return traceStore.subscribe((e) => setEntries([...e]));
  }, []);

  function currentSectionEntry() {
    if (route.kind !== 'section') return null;
    return { spec: route.spec, anchor: route.anchor };
  }

  function currentStepEntry() {
    if (route.kind !== 'section') return null;
    if (!selectedStepPath || selectedStepPath.length === 0) return null;
    return { spec: route.spec, anchor: route.anchor, step_path: selectedStepPath };
  }

  function addSection() {
    const entry = currentSectionEntry();
    if (entry) traceStore.add(entry);
  }

  function addStep() {
    const entry = currentStepEntry();
    if (entry) traceStore.add(entry);
  }

  function copyMarkdown() {
    const md = traceToMarkdown([...entries], getTitleCache());
    if (!md) return;

    if (typeof navigator !== 'undefined' && navigator.clipboard) {
      navigator.clipboard.writeText(md).then(() => {
        setCopyFeedback('Copied!');
        setTimeout(() => setCopyFeedback(''), 2000);
      }).catch(() => fallbackCopy(md));
    } else {
      fallbackCopy(md);
    }
  }

  function fallbackCopy(text: string, feedback = 'Copied!') {
    if (textareaRef.current) {
      textareaRef.current.value = text;
      textareaRef.current.select();
      document.execCommand('copy');
      setCopyFeedback(feedback);
      setTimeout(() => setCopyFeedback(''), 2000);
    }
  }

  async function copyShareLink() {
    try {
      const payload = await encodeTrace([...entries]);
      const url = location.origin + location.pathname + '#/trace/' + payload;
      if (typeof navigator !== 'undefined' && navigator.clipboard) {
        navigator.clipboard.writeText(url).then(() => {
          setCopyFeedback('Link copied!');
          setTimeout(() => setCopyFeedback(''), 2000);
        }).catch(() => fallbackCopy(url, 'Link copied!'));
      } else {
        fallbackCopy(url, 'Link copied!');
      }
    } catch {
      setCopyFeedback('Failed to encode.');
      setTimeout(() => setCopyFeedback(''), 2000);
    }
  }

  const md = traceToMarkdown([...entries], getTitleCache());
  const sectionEntry = currentSectionEntry();
  const stepEntry = currentStepEntry();

  return (
    <div class="trace-panel" role="complementary" aria-label="Trace recorder">
      <div class="trace-panel-header">
        <span class="trace-panel-title">Trace recorder — {entries.length} step{entries.length !== 1 ? 's' : ''}</span>
        <button class="trace-panel-close" onClick={onClose} aria-label="Close trace panel">✕</button>
      </div>

      <div class="trace-panel-body">
        {entries.length === 0 && (
          <div class="trace-empty">No steps recorded yet. Navigate to a section and press <kbd>t</kbd> or use "Add section".</div>
        )}

        {entries.map((e, i) => (
          <EntryCard
            key={e.id}
            entry={e}
            index={i}
            total={entries.length}
          />
        ))}

        {md && (
          <details class="trace-markdown-details">
            <summary>Markdown preview</summary>
            <pre class="trace-markdown-pre">{md}</pre>
          </details>
        )}
      </div>

      <textarea
        ref={textareaRef}
        class="trace-clipboard-ta"
        aria-hidden="true"
        tabIndex={-1}
        readOnly
      />

      <div class="trace-panel-actions">
        {sectionEntry && (
          <button class="trace-action-btn primary" onClick={addSection}>
            Add section
          </button>
        )}
        {stepEntry && (
          <button class="trace-action-btn primary" onClick={addStep}>
            Add step {stepEntry.step_path!.join('.')}
          </button>
        )}
        {entries.length > 0 && (
          <>
            <button class="trace-action-btn" onClick={copyMarkdown}>
              Copy markdown
            </button>
            <button class="trace-action-btn" onClick={copyShareLink}>
              Share link
            </button>
            {copyFeedback && <span class="trace-copy-feedback">{copyFeedback}</span>}
            <button class="trace-action-btn danger" onClick={() => traceStore.clear()}>
              Clear
            </button>
          </>
        )}
      </div>
    </div>
  );
}

interface EntryCardProps {
  entry: TraceEntry;
  index: number;
  total: number;
}

function EntryCard({ entry, index, total }: EntryCardProps) {
  const [note, setNote] = useState(entry.note ?? '');

  function commitNote() {
    traceStore.setNote(index, note);
  }

  const key = `${entry.spec}#${entry.anchor}${entry.step_path ? ' step ' + entry.step_path.join('.') : ''}`;

  return (
    <div class="trace-entry">
      <div class="trace-entry-header">
        <span class="trace-entry-key" title={key}>{index + 1}. {key}</span>
        <div class="trace-entry-controls">
          <button
            class="trace-icon-btn"
            onClick={() => traceStore.move(index, index - 1)}
            disabled={index === 0}
            aria-label="Move up"
            title="Move up"
          >↑</button>
          <button
            class="trace-icon-btn"
            onClick={() => traceStore.move(index, index + 1)}
            disabled={index === total - 1}
            aria-label="Move down"
            title="Move down"
          >↓</button>
          <button
            class="trace-icon-btn danger"
            onClick={() => traceStore.remove(index)}
            aria-label="Remove"
            title="Remove"
          >✕</button>
        </div>
      </div>
      <textarea
        class="trace-entry-note"
        placeholder="Add a note…"
        rows={2}
        value={note}
        onInput={(e) => setNote((e.target as HTMLTextAreaElement).value)}
        onBlur={commitNote}
      />
    </div>
  );
}
