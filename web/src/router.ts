// Hash router. All routes live under the fragment (#) so GitHub Pages serves
// the same index.html regardless of path.

export type Route =
  | { kind: 'landing' }
  | { kind: 'headings'; spec: string }
  | { kind: 'section'; spec: string; anchor: string; step?: number[] }
  | { kind: 'search'; q: string; spec?: string }
  | { kind: 'trace'; payload: string }
  | { kind: 'resolve'; url: string }
  | { kind: 'not_found' };

// Parse location.hash (e.g. "#/HTML/navigate") into a Route.
// The leading "#" is included in the input; anything after "#/" is the path.
// A full spec URL (starting with http:// or https://) is passed through as
// resolve so the section view can redirect to the canonical #/SPEC/anchor route.
export function parseRoute(hash: string): Route {
  // Normalise: strip leading "#" to get the raw fragment.
  const fragment = hash.startsWith('#') ? hash.slice(1) : hash;

  if (fragment === '' || fragment === '/') {
    return { kind: 'landing' };
  }

  if (!fragment.startsWith('/')) {
    return { kind: 'not_found' };
  }

  // Everything after the leading "/".
  const path = fragment.slice(1);

  if (path === '') {
    return { kind: 'landing' };
  }

  if (path.startsWith('https://') || path.startsWith('http://')) {
    return { kind: 'resolve', url: path };
  }

  if (path === 'search' || path.startsWith('search?')) {
    const qmarkIdx = path.indexOf('?');
    const qs = qmarkIdx === -1 ? '' : path.slice(qmarkIdx + 1);
    const params = new URLSearchParams(qs);
    const q = params.get('q') ?? '';
    const spec = params.get('spec') ?? undefined;
    return { kind: 'search', q, spec };
  }

  if (path.startsWith('trace/')) {
    return { kind: 'trace', payload: path.slice('trace/'.length) };
  }

  // Split spec/anchor on the first "/".
  const slashIdx = path.indexOf('/');
  if (slashIdx === -1) {
    return { kind: 'headings', spec: path };
  }

  const spec = path.slice(0, slashIdx);
  const remaining = path.slice(slashIdx + 1);
  const qmarkIdx = remaining.indexOf('?');
  const anchor = qmarkIdx === -1 ? remaining : remaining.slice(0, qmarkIdx);
  const qs = qmarkIdx === -1 ? '' : remaining.slice(qmarkIdx + 1);
  const params = new URLSearchParams(qs);
  const stepStr = params.get('step');
  let step: number[] | undefined;
  if (stepStr) {
    const parts = stepStr.split('.').map(Number);
    if (parts.length > 0 && parts.every((n) => Number.isInteger(n) && n > 0)) {
      step = parts;
    }
  }
  return step ? { kind: 'section', spec, anchor, step } : { kind: 'section', spec, anchor };
}

// Serialise a Route back to a hash string.
export function routeToHash(route: Route): string {
  switch (route.kind) {
    case 'landing':
      return '#/';
    case 'headings':
      return `#/${route.spec}`;
    case 'section':
      return route.step && route.step.length > 0
        ? `#/${route.spec}/${route.anchor}?step=${route.step.join('.')}`
        : `#/${route.spec}/${route.anchor}`;
    case 'search': {
      const qs = new URLSearchParams({ q: route.q });
      if (route.spec !== undefined) qs.set('spec', route.spec);
      return `#/search?${qs.toString()}`;
    }
    case 'trace':
      return `#/trace/${route.payload}`;
    case 'resolve':
      return `#/${route.url}`;
    case 'not_found':
      return '#/';
  }
}
