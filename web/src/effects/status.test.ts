import { describe, it, expect } from 'vitest';
import { describeStatus } from './status';
import type { EffectsStatus } from '../api/types';

const baseReady = (coverage: 'complete' | 'partial', extra?: Partial<Extract<EffectsStatus, { state: 'ready' }>>): EffectsStatus => ({
  state: 'ready',
  semantics: 'may',
  coverage,
  issues: [],
  omitted: 0,
  analysis_id: 'an_abc',
  ...extra,
});

describe('describeStatus', () => {
  it('ready+complete → ok, "Analysis complete"', () => {
    const result = describeStatus(baseReady('complete'));
    expect(result.tone).toBe('ok');
    expect(result.headline).toBe('Analysis complete');
    expect(result.detail).toBeUndefined();
  });

  it('ready+partial → warn, "Partial analysis" with issue codes humanised in detail', () => {
    const result = describeStatus(baseReady('partial', { issues: ['unresolved_invocation', 'missing_spec'] }));
    expect(result.tone).toBe('warn');
    expect(result.headline).toBe('Partial analysis');
    expect(result.detail).toContain('lower bound');
    expect(result.detail).toContain('some links could not be resolved to an algorithm');
    expect(result.detail).toContain('some called specs are not indexed');
  });

  it('pending → muted, "Analysis not prepared yet"', () => {
    const result = describeStatus({ state: 'pending', semantics: 'may', issues: [], omitted: 0 });
    expect(result.tone).toBe('muted');
    expect(result.headline).toBe('Analysis not prepared yet');
  });

  it('unavailable → muted, "No prepared analysis for this section"', () => {
    const result = describeStatus({ state: 'unavailable', semantics: 'may', issues: [], omitted: 0 });
    expect(result.tone).toBe('muted');
    expect(result.headline).toBe('No prepared analysis for this section');
  });

  it('error → warn', () => {
    const result = describeStatus({ state: 'error', semantics: 'may', issues: [], omitted: 0 });
    expect(result.tone).toBe('warn');
  });

  it('disabled → muted', () => {
    const result = describeStatus({ state: 'disabled', semantics: 'may', issues: [], omitted: 0 });
    expect(result.tone).toBe('muted');
  });

  it('omitted > 0 appends "N effects omitted" to detail', () => {
    const result = describeStatus(baseReady('complete', { omitted: 3 }));
    expect(result.detail).toContain('3 effects omitted');
  });

  it('omitted > 0 on partial appends to detail alongside issue codes', () => {
    const result = describeStatus(baseReady('partial', { issues: ['missing_anchor'], omitted: 2 }));
    expect(result.detail).toContain('anchors that were not found');
    expect(result.detail).toContain('2 effects omitted');
  });
});
