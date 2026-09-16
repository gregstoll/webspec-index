import { describe, it, expect } from 'vitest';
import { toMermaid } from './mermaid';
import type { DiagramGraph } from './model';

describe('toMermaid', () => {
  it('produces the exact expected string for a 3-node graph', () => {
    const g: DiagramGraph = {
      nodes: [
        { id: 'check', label: 'Check condition', kind: 'branch' },
        { id: 'done', label: 'Done', kind: 'terminal' },
        { id: 'ext', label: 'External', kind: 'external' },
      ],
      edges: [
        { from: 'check', to: 'done', kind: 'unknown' },
        { from: 'check', to: 'ext', kind: 'next', label: 'yes' },
      ],
    };

    const expected = [
      'flowchart TD',
      '  check{"Check condition"}',
      '  done(["Done"])',
      '  ext[["External"]]',
      '  check -.-> done',
      '  check -- "yes" --> ext',
    ].join('\n');

    expect(toMermaid(g)).toBe(expected);
  });

  it('sanitises node ids containing non-alphanumeric characters', () => {
    const g: DiagramGraph = {
      nodes: [
        { id: 'HTML#navigate', label: 'navigate', kind: 'section' },
        { id: 'DOM#concept-node', label: 'node', kind: 'step' },
      ],
      edges: [{ from: 'HTML#navigate', to: 'DOM#concept-node', kind: 'reference' }],
    };
    const out = toMermaid(g);
    expect(out).not.toContain('#');
    expect(out).toContain('HTML_navigate');
    expect(out).toContain('DOM_concept_node');
  });

  it('escapes double quotes in labels', () => {
    const g: DiagramGraph = {
      nodes: [{ id: 'a', label: 'Say "hello"', kind: 'step' }],
      edges: [],
    };
    const out = toMermaid(g);
    expect(out).not.toMatch(/"Say "hello"/);
    expect(out).toContain('#quot;');
  });
});
