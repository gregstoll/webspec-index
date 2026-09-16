import type { EffectSummary, EffectSummaryResult, ExplainEffectsResult } from '../api/types';

export interface StepEffect {
  effect: EffectSummary;
  /** `direct` when the effect's own site is this step; `path` when a call in this step leads to it. */
  via: 'direct' | 'path';
}

export type StepEffects = Map<string, StepEffect[]>;

export function stepKey(path: number[]): string {
  return path.join('.');
}

/**
 * Attributes each effect of a section to the steps of that section where it originates.
 * Effects located in the section itself attach to their own step. Effects reached through
 * other algorithms attach to the step whose call starts a witness path, so they need the
 * explanation; without it only direct effects are attributed.
 */
export function stepEffects(
  summary: EffectSummaryResult,
  explain: ExplainEffectsResult | null,
  subject: { spec: string; anchor: string },
): StepEffects {
  const out: StepEffects = new Map();
  const seen = new Set<string>();

  function add(path: number[], effect: EffectSummary, via: StepEffect['via']) {
    const key = stepKey(path);
    const dedupe = `${key}|${effect.id}`;
    if (seen.has(dedupe)) return;
    seen.add(dedupe);
    const list = out.get(key) ?? [];
    list.push({ effect, via });
    out.set(key, list);
  }

  const isHere = (loc: { spec: string; anchor: string }) =>
    loc.spec === subject.spec && loc.anchor === subject.anchor;

  for (const effect of summary.effects) {
    const locations = [effect.location, ...(effect.other_locations ?? [])];
    for (const loc of locations) {
      if (loc && loc.step_path && isHere(loc)) add(loc.step_path, effect, 'direct');
    }
  }

  if (explain) {
    const byId = new Map(summary.effects.map((e) => [e.id, e]));
    for (const explanation of explain.explanations) {
      const effect = byId.get(explanation.effect_id);
      if (!effect) continue;
      for (const witness of explanation.witnesses) {
        const first = witness.hops[0];
        if (!first) continue;
        // The hop's `from` names the subject algorithm as a whole; the call site
        // carries the step where the invocation happens.
        const origin = first.site.subject.step_path ? first.site.subject : first.from;
        if (origin.step_path && isHere(origin)) add(origin.step_path, effect, 'path');
      }
    }
  }

  for (const list of out.values()) {
    list.sort((a, b) => (a.via === b.via ? 0 : a.via === 'direct' ? -1 : 1));
  }
  return out;
}
