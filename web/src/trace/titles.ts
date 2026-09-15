import type { QueryResult } from '../api/types';

export type TitleEntry = { title: string; url: string };

// Singleton cache: "SPEC#anchor" -> { title, url }
// Populated by remember() on every successful query response.
const _cache = new Map<string, TitleEntry>();

export function getTitleCache(): Map<string, TitleEntry> {
  return _cache;
}

// Records the title and URL of every successful query response so the trace panel can
// resolve entries without re-querying.
export function remember(result: QueryResult): void {
  const key = `${result.spec}#${result.anchor}`;
  _cache.set(key, { title: result.title ?? result.anchor, url: result.url });
}
