import type { TraceEntry } from './store';

export type TitleCache = Map<string, { title: string; url: string }>;

function escapeMd(text: string): string {
  return text.replace(/[[\]`]/g, '\\$&');
}

export function traceToMarkdown(entries: TraceEntry[], titles?: TitleCache): string {
  if (entries.length === 0) return '';

  function entryKey(e: TraceEntry): string {
    return `${e.spec}#${e.anchor}`;
  }

  function entryUrl(e: TraceEntry): string {
    const cached = titles?.get(entryKey(e));
    return cached?.url ?? `#/${e.spec}/${e.anchor}`;
  }

  function entryLabel(e: TraceEntry): string {
    const base = entryKey(e);
    if (e.step_path && e.step_path.length > 0) {
      return `${base} step ${e.step_path.join('.')}`;
    }
    return base;
  }

  const firstKey = entryKey(entries[0]);
  const lastKey = entryKey(entries[entries.length - 1]);

  const lines: string[] = [
    `# trace: \`${firstKey}\` -> \`${lastKey}\``,
    '',
    `Recorded ${entries.length} step(s).`,
    '',
  ];

  for (let i = 0; i < entries.length; i++) {
    const e = entries[i];
    const label = entryLabel(e);
    const url = entryUrl(e);
    const note = e.note ? ` — ${escapeMd(e.note)}` : '';
    lines.push(`${i + 1}) [\`${label}\`](${url})${note}`);
  }

  return lines.join('\n');
}
