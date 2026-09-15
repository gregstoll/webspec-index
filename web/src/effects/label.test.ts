import { describe, it, expect } from 'vitest';
import { effectLabel } from './label';
import type { EffectSummary } from '../api/types';

const base = (overrides: Partial<EffectSummary> = {}): EffectSummary => ({
  id: 'ef_test',
  kind: 'fire_event',
  params: { name: 'load', target: null },
  execution: ['inline'],
  ...overrides,
});

describe('effectLabel', () => {
  it('humanises kind and renders non-null params', () => {
    expect(effectLabel(base())).toBe('may fire event name=load');
  });

  it('separate-only execution adds "(separate)" suffix', () => {
    expect(effectLabel(base({ execution: ['separate'] }))).toBe('may fire event name=load (separate)');
  });

  it('inline+separate execution adds "(inline or separate)" suffix', () => {
    expect(effectLabel(base({ execution: ['inline', 'separate'] }))).toBe('may fire event name=load (inline or separate)');
  });

  it('humanises kind with hyphens', () => {
    expect(effectLabel(base({ kind: 'post-task', params: {}, execution: ['inline'] }))).toBe('may post task');
  });

  it('omits null params and renders remaining', () => {
    expect(effectLabel(base({ params: { name: 'load', target: null, bubbles: true } }))).toBe('may fire event bubbles=true name=load');
  });

  it('no params renders no trailing space', () => {
    expect(effectLabel(base({ params: {} }))).toBe('may fire event');
  });

  it('unknown execution treated as lacking inline → "(separate)"', () => {
    expect(effectLabel(base({ execution: ['unknown'] }))).toBe('may fire event name=load (separate)');
  });
});
