import { describe, it, expect } from 'vitest';
import { effectPaths } from './effectPaths';
import type { EffectExplanation, EffectSummary, Subject, Witness } from '../api/types';

const subject: Subject = { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc' };

const effect: EffectSummary = {
  id: 'effect-1',
  kind: 'fire_event',
  params: { name: 'load' },
  execution: ['inline'],
};

function makeWitness(hops: Witness['hops']): Witness {
  return { hops, terminal_evidence: [], path_feasibility: 'unchecked', issues: [] };
}

const twoHopWitness = makeWitness([
  {
    from: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc', step_path: [1] },
    to: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc' },
    relation: 'invoke',
    site: {
      id: 'site-1',
      subject,
      url: 'https://html.spec.whatwg.org/#navigate',
      step_text: 'Fire an event named load at the document',
    },
    context: [],
  },
  {
    from: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc' },
    to: { spec: 'HTML', anchor: 'concept-event-fire', snapshot_sha: 'abc' },
    relation: 'invoke',
    site: {
      id: 'site-2',
      subject: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc' },
      url: 'https://html.spec.whatwg.org/#fire-an-event',
      step_text: 'Dispatch event at target',
    },
    context: [],
  },
]);

const explanation: EffectExplanation = {
  effect_id: 'effect-1',
  witnesses: [twoHopWitness],
  witnesses_truncated: false,
  issues: [],
};

describe('effectPaths', () => {
  it('two hops produce three hop nodes plus one effect node', () => {
    const graph = effectPaths(explanation, effect, subject);
    expect(graph.nodes.length).toBe(4);
    const kinds = graph.nodes.map((n) => n.kind).sort();
    expect(kinds).toContain('step');
    expect(kinds).toContain('section');
    expect(kinds).toContain('effect');
  });

  it('two hops produce two call edges and one path edge', () => {
    const graph = effectPaths(explanation, effect, subject);
    const callEdges = graph.edges.filter((e) => e.kind === 'call');
    const pathEdges = graph.edges.filter((e) => e.kind === 'path');
    expect(callEdges.length).toBe(2);
    expect(pathEdges.length).toBe(1);
  });

  it('step endpoint kind: from-node with step_path in subject section is step', () => {
    const graph = effectPaths(explanation, effect, subject);
    const stepNode = graph.nodes.find((n) => n.id === 'HTML#navigate@1');
    expect(stepNode).toBeTruthy();
    expect(stepNode!.kind).toBe('step');
    expect(stepNode!.label).toBe('step 1');
  });

  it('section endpoint kind: same spec, different anchor, no step_path', () => {
    const graph = effectPaths(explanation, effect, subject);
    const sectionNode = graph.nodes.find((n) => n.id === 'HTML#fire-an-event');
    expect(sectionNode).toBeTruthy();
    expect(sectionNode!.kind).toBe('section');
    expect(sectionNode!.label).toBe('HTML#fire-an-event');
  });

  it('external endpoint kind: different spec', () => {
    const externalWitness = makeWitness([
      {
        from: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc', step_path: [2] },
        to: { spec: 'DOM', anchor: 'concept-event-fire', snapshot_sha: 'def' },
        relation: 'invoke',
        site: {
          id: 'site-x',
          subject,
          url: 'https://html.spec.whatwg.org/#navigate',
        },
        context: [],
      },
    ]);
    const externalExpl: EffectExplanation = {
      effect_id: 'effect-1',
      witnesses: [externalWitness],
      witnesses_truncated: false,
      issues: [],
    };
    const graph = effectPaths(externalExpl, effect, subject);
    const extNode = graph.nodes.find((n) => n.id === 'DOM#concept-event-fire');
    expect(extNode).toBeTruthy();
    expect(extNode!.kind).toBe('external');
  });

  it('relation label is humanised (underscores to spaces)', () => {
    const candidateWitness = makeWitness([
      {
        from: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc', step_path: [3] },
        to: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc' },
        relation: 'candidate_invoke',
        site: {
          id: 'site-r',
          subject,
          url: 'https://html.spec.whatwg.org/#navigate',
        },
        context: [],
      },
    ]);
    const expl: EffectExplanation = {
      effect_id: 'effect-1',
      witnesses: [candidateWitness],
      witnesses_truncated: false,
      issues: [],
    };
    const graph = effectPaths(expl, effect, subject);
    const callEdge = graph.edges.find((e) => e.kind === 'call');
    expect(callEdge?.label).toBe('candidate invoke');
  });

  it('boundary execution appended to edge label', () => {
    const boundaryWitness = makeWitness([
      {
        from: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc', step_path: [4] },
        to: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc' },
        relation: 'invoke',
        site: {
          id: 'site-b',
          subject,
          url: 'https://html.spec.whatwg.org/#navigate',
        },
        context: [],
        boundary: {
          execution: 'separate',
          operation_site_id: 'op-1',
        },
      },
    ]);
    const expl: EffectExplanation = {
      effect_id: 'effect-1',
      witnesses: [boundaryWitness],
      witnesses_truncated: false,
      issues: [],
    };
    const graph = effectPaths(expl, effect, subject);
    const callEdge = graph.edges.find((e) => e.kind === 'call');
    expect(callEdge?.label).toBe('invoke · separate');
  });

  it('two witnesses sharing a node merge via mergeGraphs', () => {
    const witnessA = makeWitness([
      {
        from: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc', step_path: [1] },
        to: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc' },
        relation: 'invoke',
        site: { id: 's1', subject, url: 'https://html.spec.whatwg.org/#navigate' },
        context: [],
      },
    ]);
    const witnessB = makeWitness([
      {
        from: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc', step_path: [5] },
        to: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc' },
        relation: 'invoke',
        site: { id: 's2', subject, url: 'https://html.spec.whatwg.org/#navigate' },
        context: [],
      },
    ]);
    const multiExpl: EffectExplanation = {
      effect_id: 'effect-1',
      witnesses: [witnessA, witnessB],
      witnesses_truncated: false,
      issues: [],
    };
    const graph = effectPaths(multiExpl, effect, subject);
    const fireEventNodes = graph.nodes.filter((n) => n.id === 'HTML#fire-an-event');
    expect(fireEventNodes.length).toBe(1);
    const effectNodes = graph.nodes.filter((n) => n.kind === 'effect');
    expect(effectNodes.length).toBe(1);
  });

  it('sublabel on from node is step_text truncated to 60 chars with ellipsis appended', () => {
    const longText = 'A'.repeat(70);
    const w = makeWitness([
      {
        from: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc', step_path: [1] },
        to: { spec: 'HTML', anchor: 'fire-an-event', snapshot_sha: 'abc' },
        relation: 'invoke',
        site: { id: 's', subject, url: 'u', step_text: longText },
        context: [],
      },
    ]);
    const expl: EffectExplanation = {
      effect_id: 'effect-1',
      witnesses: [w],
      witnesses_truncated: false,
      issues: [],
    };
    const graph = effectPaths(expl, effect, subject);
    const fromNode = graph.nodes.find((n) => n.id === 'HTML#navigate@1');
    expect(fromNode?.sublabel).toBe('A'.repeat(60) + '…');
  });
});
