import { describe, it, expect } from 'vitest';
import { traceToMarkdown } from './markdown';
import type { TraceEntry } from './store';
import type { TitleCache } from './markdown';

const A: TraceEntry = { id: 'a', spec: 'HTML', anchor: 'navigate', note: 'note text' };
const B: TraceEntry = { id: 'b', spec: 'HTML', anchor: 'browsing' };
const C: TraceEntry = { id: 'c', spec: 'DOM', anchor: 'concept-tree' };

const titles: TitleCache = new Map([
  ['HTML#navigate', { title: 'navigate', url: 'https://html.spec.whatwg.org/#navigate' }],
  ['HTML#browsing', { title: 'browsing context', url: 'https://html.spec.whatwg.org/#browsing' }],
  ['DOM#concept-tree', { title: 'tree', url: 'https://dom.spec.whatwg.org/#concept-tree' }],
]);

describe('traceToMarkdown', () => {
  it('returns empty string for empty entries', () => {
    expect(traceToMarkdown([])).toBe('');
  });

  it('renders the exact markdown shape for 3-entry fixture', () => {
    const md = traceToMarkdown([A, B, C], titles);
    const expected = [
      '# trace: `HTML#navigate` -> `DOM#concept-tree`',
      '',
      'Recorded 3 step(s).',
      '',
      '1) [`HTML#navigate`](https://html.spec.whatwg.org/#navigate) — note text',
      '2) [`HTML#browsing`](https://html.spec.whatwg.org/#browsing)',
      '3) [`DOM#concept-tree`](https://dom.spec.whatwg.org/#concept-tree)',
    ].join('\n');
    expect(md).toBe(expected);
  });

  it('uses fallback URL when no title cache entry', () => {
    const md = traceToMarkdown([A, C]);
    expect(md).toContain('(#/HTML/navigate)');
    expect(md).toContain('(#/DOM/concept-tree)');
  });

  it('falls back to site URL when cache is provided but key is missing', () => {
    const partialCache: TitleCache = new Map([
      ['HTML#navigate', { title: 'navigate', url: 'https://html.spec.whatwg.org/#navigate' }],
    ]);
    const md = traceToMarkdown([A, C], partialCache);
    expect(md).toContain('(https://html.spec.whatwg.org/#navigate)');
    expect(md).toContain('(#/DOM/concept-tree)');
  });

  it('single entry: first and last are the same', () => {
    const md = traceToMarkdown([A], titles);
    expect(md.startsWith('# trace: `HTML#navigate` -> `HTML#navigate`')).toBe(true);
    expect(md).toContain('Recorded 1 step(s).');
  });

  it('includes step_path in label', () => {
    const entry: TraceEntry = { id: 'd', spec: 'HTML', anchor: 'navigate', step_path: [3, 1] };
    const md = traceToMarkdown([entry, C], titles);
    expect(md).toContain('`HTML#navigate step 3.1`');
  });

  it('omits note when not set', () => {
    const md = traceToMarkdown([B], titles);
    expect(md).not.toContain(' — ');
  });

  it('first line uses SPEC#anchor keys not titles', () => {
    const md = traceToMarkdown([A, C], titles);
    expect(md.startsWith('# trace: `HTML#navigate` -> `DOM#concept-tree`')).toBe(true);
  });

  it('escapes ] and backtick in note to avoid breaking markdown syntax', () => {
    const entry: TraceEntry = { id: 'e', spec: 'HTML', anchor: 'navigate', note: 'see [step 1] and `foo`' };
    const md = traceToMarkdown([entry], titles);
    expect(md).toContain('\\[step 1\\]');
    expect(md).toContain('\\`foo\\`');
  });
});
