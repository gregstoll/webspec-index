import { describe, it, expect } from 'vitest';
import { routeForQuery } from './dispatch';

describe('routeForQuery', () => {
  it('returns landing for empty string', () => {
    expect(routeForQuery('')).toEqual({ kind: 'landing' });
  });

  it('returns landing for whitespace-only input', () => {
    expect(routeForQuery('   ')).toEqual({ kind: 'landing' });
  });

  it('returns resolve for an https URL', () => {
    expect(routeForQuery('https://html.spec.whatwg.org/#navigate')).toEqual({
      kind: 'resolve',
      url: 'https://html.spec.whatwg.org/#navigate',
    });
  });

  it('returns resolve for an http URL', () => {
    expect(routeForQuery('http://example.com/spec#foo')).toEqual({
      kind: 'resolve',
      url: 'http://example.com/spec#foo',
    });
  });

  it('returns section for SPEC#anchor pattern', () => {
    expect(routeForQuery('HTML#navigate')).toEqual({
      kind: 'section',
      spec: 'HTML',
      anchor: 'navigate',
    });
  });

  it('returns section for lowercase spec with dots', () => {
    expect(routeForQuery('CSS-GRID.1#track-sizing')).toEqual({
      kind: 'section',
      spec: 'CSS-GRID.1',
      anchor: 'track-sizing',
    });
  });

  it('returns search for a plain keyword', () => {
    expect(routeForQuery('navigate')).toEqual({ kind: 'search', q: 'navigate' });
  });

  it('returns search for multi-word query', () => {
    expect(routeForQuery('tree order')).toEqual({ kind: 'search', q: 'tree order' });
  });

  it('trims whitespace before classifying', () => {
    expect(routeForQuery('  HTML#navigate  ')).toEqual({
      kind: 'section',
      spec: 'HTML',
      anchor: 'navigate',
    });
  });
});
