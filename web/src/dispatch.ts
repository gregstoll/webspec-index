import { type Route, routeToHash } from './router';

// Produce a Route from raw user input (search box or landing form).
// - Full URLs (http/https) → resolve route for server-side lookup.
// - SPEC#anchor → direct section route.
// - Anything else → full-text search.
// Returns { kind: 'landing' } for empty input so callers can no-op.
export function routeForQuery(input: string): Route {
  const q = input.trim();
  if (!q) return { kind: 'landing' };
  if (q.startsWith('http://') || q.startsWith('https://')) {
    return { kind: 'resolve', url: q };
  }
  if (/^[A-Za-z0-9._-]+#.+$/.test(q)) {
    const hashIdx = q.indexOf('#');
    return { kind: 'section', spec: q.slice(0, hashIdx), anchor: q.slice(hashIdx + 1) };
  }
  return { kind: 'search', q };
}

// Navigate the page to the hash corresponding to the user's input.
// No-ops on empty input.
export function navigateForQuery(input: string): void {
  const route = routeForQuery(input);
  if (route.kind === 'landing') return;
  window.location.hash = routeToHash(route);
}
