import { describe, it, expect } from 'vitest';
import { parseRoute, routeToHash } from './router';

describe('parseRoute', () => {
  it('returns landing for "#/"', () => {
    expect(parseRoute('#/')).toEqual({ kind: 'landing' });
  });

  it('returns landing for empty hash', () => {
    expect(parseRoute('#')).toEqual({ kind: 'landing' });
  });

  it('returns landing for empty string', () => {
    expect(parseRoute('')).toEqual({ kind: 'landing' });
  });

  it('returns headings for "#/HTML"', () => {
    expect(parseRoute('#/HTML')).toEqual({ kind: 'headings', spec: 'HTML' });
  });

  it('returns headings for "#/DOM"', () => {
    expect(parseRoute('#/DOM')).toEqual({ kind: 'headings', spec: 'DOM' });
  });

  it('treats a spec whose name starts with "search" as headings, not search', () => {
    expect(parseRoute('#/searchfoo')).toEqual({ kind: 'headings', spec: 'searchfoo' });
    expect(parseRoute('#/search')).toEqual({ kind: 'search', q: '', spec: undefined });
  });

  it('returns section for "#/HTML/navigate"', () => {
    expect(parseRoute('#/HTML/navigate')).toEqual({ kind: 'section', spec: 'HTML', anchor: 'navigate' });
  });

  it('returns section for "#/DOM/concept-tree"', () => {
    expect(parseRoute('#/DOM/concept-tree')).toEqual({ kind: 'section', spec: 'DOM', anchor: 'concept-tree' });
  });

  it('returns search with query and spec for "#/search?q=navigate&spec=HTML"', () => {
    expect(parseRoute('#/search?q=navigate&spec=HTML')).toEqual({ kind: 'search', q: 'navigate', spec: 'HTML' });
  });

  it('returns search without spec when spec param absent', () => {
    expect(parseRoute('#/search?q=foo')).toEqual({ kind: 'search', q: 'foo', spec: undefined });
  });

  it('returns resolve for "#/https://html.spec.whatwg.org/#navigate"', () => {
    expect(parseRoute('#/https://html.spec.whatwg.org/#navigate')).toEqual({
      kind: 'resolve',
      url: 'https://html.spec.whatwg.org/#navigate',
    });
  });

  it('returns resolve for an http URL', () => {
    expect(parseRoute('#/http://example.com/spec#anchor')).toEqual({
      kind: 'resolve',
      url: 'http://example.com/spec#anchor',
    });
  });

  it('returns trace for "#/trace/abc123"', () => {
    expect(parseRoute('#/trace/abc123')).toEqual({ kind: 'trace', payload: 'abc123' });
  });

  it('returns not_found for a fragment without leading slash', () => {
    expect(parseRoute('#HTML')).toEqual({ kind: 'not_found' });
  });

  it('parses ?step=1 on a section route', () => {
    expect(parseRoute('#/HTML/navigate?step=1')).toEqual({
      kind: 'section', spec: 'HTML', anchor: 'navigate', step: [1],
    });
  });

  it('parses ?step=3.1 as a nested path', () => {
    expect(parseRoute('#/HTML/navigate?step=3.1')).toEqual({
      kind: 'section', spec: 'HTML', anchor: 'navigate', step: [3, 1],
    });
  });

  it('ignores a malformed step param (zero)', () => {
    expect(parseRoute('#/HTML/navigate?step=0')).toEqual({
      kind: 'section', spec: 'HTML', anchor: 'navigate',
    });
  });

  it('ignores a malformed step param (non-numeric)', () => {
    expect(parseRoute('#/HTML/navigate?step=abc')).toEqual({
      kind: 'section', spec: 'HTML', anchor: 'navigate',
    });
  });
});

describe('routeToHash', () => {
  it('landing → "#/"', () => {
    expect(routeToHash({ kind: 'landing' })).toBe('#/');
  });

  it('headings → "#/SPEC"', () => {
    expect(routeToHash({ kind: 'headings', spec: 'HTML' })).toBe('#/HTML');
  });

  it('section → "#/SPEC/anchor"', () => {
    expect(routeToHash({ kind: 'section', spec: 'HTML', anchor: 'navigate' })).toBe('#/HTML/navigate');
  });

  it('section with step → "#/SPEC/anchor?step=N"', () => {
    expect(routeToHash({ kind: 'section', spec: 'HTML', anchor: 'navigate', step: [1] })).toBe('#/HTML/navigate?step=1');
  });

  it('section with nested step → "#/SPEC/anchor?step=N.M"', () => {
    expect(routeToHash({ kind: 'section', spec: 'HTML', anchor: 'navigate', step: [3, 1] })).toBe('#/HTML/navigate?step=3.1');
  });

  it('search with spec → "#/search?q=...&spec=..."', () => {
    expect(routeToHash({ kind: 'search', q: 'navigate', spec: 'HTML' })).toBe('#/search?q=navigate&spec=HTML');
  });

  it('search without spec → "#/search?q=..."', () => {
    expect(routeToHash({ kind: 'search', q: 'foo' })).toBe('#/search?q=foo');
  });

  it('trace → "#/trace/<payload>"', () => {
    expect(routeToHash({ kind: 'trace', payload: 'abc123' })).toBe('#/trace/abc123');
  });

  it('resolve → "#/<url>"', () => {
    expect(routeToHash({ kind: 'resolve', url: 'https://html.spec.whatwg.org/#navigate' })).toBe(
      '#/https://html.spec.whatwg.org/#navigate',
    );
  });

  it('not_found → "#/"', () => {
    expect(routeToHash({ kind: 'not_found' })).toBe('#/');
  });
});

describe('round-trip: routeToHash → parseRoute', () => {
  it('section round-trips', () => {
    const route = { kind: 'section' as const, spec: 'HTML', anchor: 'navigate' };
    expect(parseRoute(routeToHash(route))).toEqual(route);
  });

  it('section with step round-trips', () => {
    const route = { kind: 'section' as const, spec: 'HTML', anchor: 'navigate', step: [3, 1] };
    expect(parseRoute(routeToHash(route))).toEqual(route);
  });

  it('headings round-trips', () => {
    const route = { kind: 'headings' as const, spec: 'DOM' };
    expect(parseRoute(routeToHash(route))).toEqual(route);
  });

  it('search round-trips', () => {
    const route = { kind: 'search' as const, q: 'navigate', spec: 'HTML' };
    expect(parseRoute(routeToHash(route))).toEqual(route);
  });
});
