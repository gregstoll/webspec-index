import { describe, it, expect } from 'vitest';
import { snippetHtml } from './Search';

describe('snippetHtml', () => {
  it('keeps FTS match markers as elements', () => {
    expect(snippetHtml('To <mark>navigate</mark>, given')).toBe('To <mark>navigate</mark>, given');
  });

  it('escapes literal HTML from spec text', () => {
    expect(snippetHtml('use <img src=x onerror=alert(1)> & <mark>script</mark>')).toBe(
      'use &lt;img src=x onerror=alert(1)&gt; &amp; <mark>script</mark>',
    );
  });
});
