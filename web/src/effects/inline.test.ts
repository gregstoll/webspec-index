import { describe, it, expect } from 'vitest';
import { stepEffects } from './inline';
import type { EffectSummary, EffectSummaryResult, ExplainEffectsResult, Witness } from '../api/types';

const here = { spec: 'HTML', anchor: 'navigate' };

function effect(id: string, location: EffectSummary['location'], other: EffectSummary['other_locations'] = []): EffectSummary {
  return { id, kind: 'fire_event', params: { name: id }, execution: ['inline'], location, other_locations: other };
}

function summary(effects: EffectSummary[]): EffectSummaryResult {
  return {
    schema_version: 1,
    subject: { ...here, snapshot_sha: 'x' },
    effects,
    effects_status: { state: 'ready', semantics: 'may', coverage: 'complete', issues: [], omitted: 0, analysis_id: 'a' },
    defined_bodies: [],
    issues: [],
  };
}

function witnessFrom(step_path: number[], spec = 'HTML', anchor = 'navigate'): Witness {
  return {
    hops: [
      {
        from: { spec, anchor, snapshot_sha: 'x', step_path },
        to: { spec: 'FETCH', anchor: 'fetch', snapshot_sha: 'y' },
        relation: 'invokes',
        site: { id: 's', subject: { spec, anchor, snapshot_sha: 'x' }, url: 'u' },
        context: [],
      },
    ],
    terminal_evidence: [],
    path_feasibility: 'feasible',
    issues: [],
  } as unknown as Witness;
}

describe('stepEffects', () => {
  it('attaches effects located in this section to their own step', () => {
    const s = summary([
      effect('load', { spec: 'HTML', anchor: 'navigate', step_path: [3], url: 'u' }),
      effect('elsewhere', { spec: 'FETCH', anchor: 'fetch', step_path: [1], url: 'u' }),
    ]);
    const map = stepEffects(s, null, here);
    expect([...map.keys()]).toEqual(['3']);
    expect(map.get('3')?.map((e) => [e.effect.id, e.via])).toEqual([['load', 'direct']]);
  });

  it('uses the first witness hop to attribute effects reached through calls', () => {
    const s = summary([effect('fetch-task', { spec: 'FETCH', anchor: 'fetch', step_path: [2], url: 'u' })]);
    const explain: ExplainEffectsResult = {
      schema_version: 1,
      subject: { ...here, snapshot_sha: 'x' },
      explanations: [
        { effect_id: 'fetch-task', witnesses: [witnessFrom([24, 5]), witnessFrom([24, 5]), witnessFrom([7], 'DOM', 'other')], witnesses_truncated: false },
      ],
    } as unknown as ExplainEffectsResult;
    const map = stepEffects(s, explain, here);
    expect([...map.keys()]).toEqual(['24.5']);
    expect(map.get('24.5')).toHaveLength(1);
    expect(map.get('24.5')?.[0].via).toBe('path');
  });

  it('lists direct effects before path effects on the same step', () => {
    const s = summary([
      effect('a', { spec: 'FETCH', anchor: 'fetch', url: 'u' }),
      effect('b', { spec: 'HTML', anchor: 'navigate', step_path: [1], url: 'u' }),
    ]);
    const explain = {
      schema_version: 1,
      subject: { ...here, snapshot_sha: 'x' },
      explanations: [{ effect_id: 'a', witnesses: [witnessFrom([1])], witnesses_truncated: false }],
    } as unknown as ExplainEffectsResult;
    expect(stepEffects(s, explain, here).get('1')?.map((e) => e.effect.id)).toEqual(['b', 'a']);
  });
});
