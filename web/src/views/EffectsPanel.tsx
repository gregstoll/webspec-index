import { useState, useEffect, useMemo } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { DefinedBody, EffectSummaryResult, ExplainEffectsResult, SubjectSelector, Witness } from '../api/types';
import { useRequest } from '../hooks/useRequest';
import { describeStatus } from '../effects/status';
import { effectLabel } from '../effects/label';
import { stepEffects, type StepEffects } from '../effects/inline';
import { Diagram } from '../diagram/Diagram';
import { effectPaths } from '../diagram/effectPaths';

interface Props {
  client: WebspecClient;
  spec: string;
  anchor: string;
  selectedStepPath?: number[];
  onClearStep?: () => void;
  /** Receives the per-step attribution of the whole section's effects as data arrives. */
  onStepEffects?: (effects: StepEffects) => void;
  /** Effect to open and scroll to, set when an inline badge is clicked. */
  focusEffectId?: string;
  /** Summary only: counts and issues, no effect list. Used while no step is selected. */
  compact?: boolean;
  /** Show only these effect ids of the section's analysis (the selected step's effects). */
  onlyEffects?: readonly string[];
}

type ExplainCache =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ok'; value: ExplainEffectsResult }
  | { kind: 'error'; message: string };

export function EffectsPanel({
  client,
  spec,
  anchor,
  selectedStepPath,
  onClearStep,
  onStepEffects,
  focusEffectId,
  compact = false,
  onlyEffects,
}: Props) {
  // With `onlyEffects` the panel shows a slice of the whole section's analysis (the
  // effects attributed to the selected step), so it queries the section, not the step.
  const stepScoped = selectedStepPath !== undefined && onlyEffects === undefined;
  const stepPathKey = stepScoped ? selectedStepPath.join(',') : '';
  const subject: SubjectSelector = stepScoped
    ? { spec, anchor, step_path: selectedStepPath }
    : { spec, anchor };

  const effectsState = useRequest<{ type: 'effects'; result: EffectSummaryResult }>(
    client,
    { type: 'effects', subject },
    [client, spec, anchor, stepPathKey],
  );

  const [open, setOpen] = useState(false);
  const [expandedEffectId, setExpandedEffectId] = useState<string | null>(null);
  const [explainCache, setExplainCache] = useState<ExplainCache>({ kind: 'idle' });
  const [pathCache, setPathCache] = useState<ExplainCache>({ kind: 'idle' });
  const [pathLimit, setPathLimit] = useState(8);

  const ready = effectsState.kind === 'ok' && effectsState.value.result.effects_status.state === 'ready';

  useEffect(() => {
    if (ready) setOpen(true);
  }, [ready]);

  useEffect(() => {
    setExplainCache({ kind: 'idle' });
    setPathCache({ kind: 'idle' });
    setPathLimit(8);
    setExpandedEffectId(null);
    setOpen(false);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [spec, anchor, stepPathKey]);

  // The witness paths say which step of this section each effect originates from, so
  // they are loaded as soon as the summary is ready rather than on demand.
  useEffect(() => {
    if (!ready) return;
    setExplainCache({ kind: 'loading' });
    let cancelled = false;
    client
      .request({ type: 'effects_explain', subject })
      .then((resp) => {
        if (cancelled) return;
        if (resp.type === 'effects_explain') {
          setExplainCache({ kind: 'ok', value: resp.result });
        } else if (resp.type === 'error') {
          setExplainCache({ kind: 'error', message: resp.message });
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) setExplainCache({ kind: 'error', message: String(e) });
      });
    return () => {
      cancelled = true;
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, spec, anchor, stepPathKey]);

  useEffect(() => {
    if (!onStepEffects || selectedStepPath) return;
    if (effectsState.kind !== 'ok') {
      onStepEffects(new Map());
      return;
    }
    const explain = explainCache.kind === 'ok' ? explainCache.value : null;
    onStepEffects(stepEffects(effectsState.value.result, explain, { spec, anchor }));
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [effectsState, explainCache, spec, anchor, stepPathKey]);

  function handleShowPaths(effectId: string) {
    setPathLimit(8);
    setExpandedEffectId(expandedEffectId === effectId ? null : effectId);
  }

  // The prepared endpoint supplies one path for inline step attribution. A
  // person opening an effect gets bounded, on-demand witness examples instead.
  useEffect(() => {
    if (!expandedEffectId || effectsState.kind !== 'ok') {
      setPathCache({ kind: 'idle' });
      return;
    }
    setPathCache({ kind: 'loading' });
    let cancelled = false;
    client
      .request({ type: 'effects_paths', subject, effect_id: expandedEffectId, limit: pathLimit })
      .then((resp) => {
        if (cancelled) return;
        if (resp.type === 'effects_paths') {
          setPathCache({ kind: 'ok', value: resp.result });
        } else if (resp.type === 'error') {
          setPathCache({ kind: 'error', message: resp.message });
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) setPathCache({ kind: 'error', message: String(error) });
      });
    return () => { cancelled = true; };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, expandedEffectId, pathLimit, spec, anchor, stepPathKey, effectsState.kind]);

  useEffect(() => {
    if (!focusEffectId || effectsState.kind !== 'ok') return;
    setOpen(true);
    setExpandedEffectId(focusEffectId);
    document.getElementById(`effect-${focusEffectId}`)?.scrollIntoView?.({ block: 'nearest' });
  }, [focusEffectId, effectsState.kind]);

  // Memoised so the Diagram keeps the same graph object between renders and does
  // not re-layout while the panel re-renders for unrelated state.
  const expandedPaths = useMemo(() => {
    if (pathCache.kind !== 'ok' || !expandedEffectId) return null;
    const explanation = pathCache.value.explanations.find((e) => e.effect_id === expandedEffectId);
    const effectEntry = pathCache.value.effects.find((e) => e.id === expandedEffectId);
    if (!explanation || !effectEntry) return null;
    return {
      explanation,
      effectEntry,
      graph: effectPaths(explanation, effectEntry, pathCache.value.subject),
    };
  }, [pathCache, expandedEffectId]);

  const subjectLabel = selectedStepPath
    ? `Effects of step ${selectedStepPath.join('.')}`
    : 'Possible effects';

  function renderStatusLine() {
    if (effectsState.kind === 'loading') {
      return <span class="effects-status-text effects-status-muted">Loading…</span>;
    }
    if (effectsState.kind === 'error') {
      return (
        <span class="effects-status-text effects-status-warn">
          {effectsState.message}
        </span>
      );
    }
    const desc = describeStatus(effectsState.value.result.effects_status);
    return (
      <span class={`effects-status-text effects-status-${desc.tone}`}>
        {desc.headline}{desc.detail ? ` — ${desc.detail}` : ''}
      </span>
    );
  }

  function renderWitnesses(effectId: string) {
    if (pathCache.kind === 'loading') {
      return <div class="effects-witnesses-loading">Loading paths…</div>;
    }
    if (pathCache.kind === 'error') {
      return <div class="effects-witnesses-error">Error: {pathCache.message}</div>;
    }
    if (pathCache.kind !== 'ok') return null;
    if (!expandedPaths || expandedEffectId !== effectId) {
      return <div class="effects-no-witnesses">No witness paths found</div>;
    }
    const { explanation, effectEntry, graph } = expandedPaths;
    const searchLimited = explanation.issues.some((issue) => issue.code === 'witness_budget');

    return (
      <div class="effects-witnesses">
        <p class="effects-witnesses-note">
          {explanation.witnesses.length} witness example{explanation.witnesses.length === 1 ? '' : 's'}.
          Paths show possible reachability; feasibility has not been checked.
        </p>
        <Diagram
          graph={graph}
          rankdir="LR"
          ariaLabel={`Paths to ${effectLabel(effectEntry)}`}
        />
        {explanation.witnesses.map((witness, index) => (
          <WitnessDetails key={index} witness={witness} index={index} />
        ))}
        {explanation.witnesses_truncated && (
          <div class="effects-witnesses-note">
            More witness examples exist.
            {!searchLimited && pathLimit < 128 && (
              <button type="button" onClick={() => setPathLimit(Math.min(pathLimit * 2, 128))}>
                Load more paths
              </button>
            )}
            {pathLimit === 128 && ' This view is limited to 128 examples.'}
          </div>
        )}
        {explanation.issues.map((issue, index) => (
          <p key={index} class="effects-witnesses-note">{issue.message}</p>
        ))}
      </div>
    );
  }

  function renderBody() {
    if (effectsState.kind !== 'ok') return null;

    const { issues, defined_bodies: definedBodies } = effectsState.value.result;
    const effects = onlyEffects
      ? effectsState.value.result.effects.filter((e) => onlyEffects.includes(e.id))
      : effectsState.value.result.effects;

    if (compact) {
      const explain = explainCache.kind === 'ok' ? explainCache.value : null;
      const steps = stepEffects(effectsState.value.result, explain, { spec, anchor }).size;
      return (
        <>
          <p class="effects-summary">
            {effects.length} effect{effects.length === 1 ? '' : 's'}
            {steps > 0 ? ` from ${steps} step${steps === 1 ? '' : 's'}` : ''}.
            {effects.length > 0 && ' Select a step to see its effects here.'}
          </p>
          {issues.length > 0 && <IssueSummary issues={issues} />}
        </>
      );
    }

    return (
      <>
        {effects.length > 0 ? (
          <ul class="effects-list">
            {effects.map((effect) => (
              <li key={effect.id} id={`effect-${effect.id}`} class="effects-item">
                <div class="effects-item-header">
                  <span class="effects-label">{effectLabel(effect)}</span>
                  <div class="effects-badges">
                    {effect.execution.map((ex) => (
                      <span key={ex} class={`badge badge-execution-${ex}`}>{ex}</span>
                    ))}
                  </div>
                  {effect.location && (
                    <a
                      href={`#/${effect.location.spec}/${effect.location.anchor}`}
                      class="effects-location-link mono"
                    >
                      {effect.location.spec}#{effect.location.anchor}
                      {effect.location.step_path
                        ? ` step ${effect.location.step_path.join('.')}`
                        : ''}
                    </a>
                  )}
                  {effect.additional_locations !== undefined && effect.additional_locations > 0 && (
                    <details class="effects-other-sites">
                      <summary>+{effect.additional_locations} other site{effect.additional_locations !== 1 ? 's' : ''}</summary>
                      <ul>
                        {(effect.other_locations ?? []).map((location, index) => (
                          <li key={index}>
                            <a href={`#/${location.spec}/${location.anchor}`}>
                              {location.spec}#{location.anchor}
                              {location.step_path ? ` step ${location.step_path.join('.')}` : ''}
                            </a>
                          </li>
                        ))}
                        {effect.additional_locations > (effect.other_locations?.length ?? 0) && (
                          <li>{effect.additional_locations - (effect.other_locations?.length ?? 0)} further sites are not listed in the summary</li>
                        )}
                      </ul>
                    </details>
                  )}
                </div>
                <button
                  class="effects-show-paths-btn"
                  onClick={() => handleShowPaths(effect.id)}
                >
                  {expandedEffectId === effect.id ? 'Hide paths' : 'Show paths'}
                </button>
                {expandedEffectId === effect.id && renderWitnesses(effect.id)}
              </li>
            ))}
          </ul>
        ) : null}
        {!onlyEffects && definedBodies.length > 0 && (
          <DefinedBodies bodies={definedBodies} />
        )}
        {issues.length > 0 && <IssueSummary issues={issues} />}
      </>
    );
  }

  return (
    <section class="effects-panel">
      <div class="effects-panel-header">
        <button
          class="effects-panel-toggle"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          <span class="effects-panel-title">{subjectLabel}</span>
          <span class="effects-panel-chevron" aria-hidden="true">{open ? '▲' : '▼'}</span>
        </button>
        {selectedStepPath && onClearStep && (
          <button class="effects-back-btn" onClick={onClearStep}>
            ← Back to whole section
          </button>
        )}
        {renderStatusLine()}
      </div>
      {open && (
        <div class="effects-panel-body">
          {renderBody()}
        </div>
      )}
    </section>
  );
}

const ISSUE_LIST_CAP = 30;

function DefinedBodies({ bodies }: { bodies: DefinedBody[] }) {
  return (
    <details class="effects-defined-bodies">
      <summary>{bodies.length} defined step bod{bodies.length === 1 ? 'y' : 'ies'}</summary>
      <ul>
        {bodies.map((body, index) => (
          <li key={body.subject.body_id ?? index}>
            <a href={`#/${body.subject.spec}/${body.subject.anchor}`}>
              {body.subject.spec}#{body.subject.anchor}
            </a>
            {body.subject.body_id && ` · ${body.subject.body_id}`}
            {' — '}{describeStatus(body.effects_status).headline}
            {body.effects.length > 0 && (
              <ul>
                {body.effects.map((effect) => (
                  <li key={effect.id}>{effectLabel(effect)}</li>
                ))}
              </ul>
            )}
          </li>
        ))}
      </ul>
    </details>
  );
}

function WitnessDetails({ witness, index }: { witness: Witness; index: number }) {
  return (
    <details class="effects-witness-details">
      <summary>Witness {index + 1} · {witness.hops.length} hop{witness.hops.length === 1 ? '' : 's'}</summary>
      {witness.hops.length > 0 && (
        <ol>
          {witness.hops.map((hop, hopIndex) => (
            <li key={hopIndex}>
              <a href={`#/${hop.from.spec}/${hop.from.anchor}`}>
                {hop.from.spec}#{hop.from.anchor}
              </a>
              {' → '}
              <a href={`#/${hop.to.spec}/${hop.to.anchor}`}>
                {hop.to.spec}#{hop.to.anchor}
              </a>
              {` (${hop.relation.replace(/_/g, ' ')})`}
              {hop.site.step_text && <p>{hop.site.step_text}</p>}
              <a href={hop.site.url} target="_blank" rel="noopener">Source step ↗</a>
            </li>
          ))}
        </ol>
      )}
      {witness.terminal_evidence.length > 0 && (
        <p>Terminal evidence: {witness.terminal_evidence.map((item) => item.rule_id).join(', ')}</p>
      )}
      <details>
        <summary>All witness data</summary>
        <pre>{JSON.stringify(witness, null, 2)}</pre>
      </details>
    </details>
  );
}

// The analysis reports one issue per unresolved site, which runs into the thousands
// for large algorithms. Show the tally per code, and the first few details on demand.
function IssueSummary({ issues }: { issues: EffectSummaryResult['issues'] }) {
  const [visible, setVisible] = useState(ISSUE_LIST_CAP);
  useEffect(() => setVisible(ISSUE_LIST_CAP), [issues]);
  const tally = new Map<string, number>();
  for (const issue of issues) tally.set(issue.code, (tally.get(issue.code) ?? 0) + 1);
  const parts = [...tally.entries()]
    .sort((a, b) => b[1] - a[1])
    .map(([code, n]) => `${code.replace(/_/g, ' ')} ×${n}`);
  return (
    <details class="effects-issues">
      <summary>
        {issues.length} analysis issue{issues.length === 1 ? '' : 's'}: {parts.join(', ')}
      </summary>
      <ul>
        {issues.slice(0, visible).map((issue, i) => (
          <li key={i} class="effects-issue">
            <code>{issue.code}</code>
            {issue.message && <> — {issue.message}</>}
            {issue.site?.url && (
              <>
                {' '}
                <a href={issue.site.url} target="_blank" rel="noopener">
                  site
                </a>
              </>
            )}
          </li>
        ))}
        {issues.length > visible && (
          <li class="effects-issue">
            <button type="button" onClick={() => setVisible(Math.min(visible + 100, issues.length))}>
              Show more issues ({issues.length - visible} remaining)
            </button>
          </li>
        )}
      </ul>
    </details>
  );
}
