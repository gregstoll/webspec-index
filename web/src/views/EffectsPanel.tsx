import { useState, useEffect } from 'preact/hooks';
import type { WebspecClient } from '../api/client';
import type { EffectSummaryResult, ExplainEffectsResult, SubjectSelector } from '../api/types';
import { useRequest } from '../hooks/useRequest';
import { describeStatus } from '../effects/status';
import { effectLabel } from '../effects/label';

interface Props {
  client: WebspecClient;
  spec: string;
  anchor: string;
  selectedStepPath?: number[];
  onClearStep?: () => void;
}

type ExplainCache =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ok'; value: ExplainEffectsResult }
  | { kind: 'error'; message: string };

export function EffectsPanel({ client, spec, anchor, selectedStepPath, onClearStep }: Props) {
  const stepPathKey = selectedStepPath ? selectedStepPath.join(',') : '';
  const subject: SubjectSelector = selectedStepPath
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

  useEffect(() => {
    if (effectsState.kind === 'ok') {
      if (effectsState.value.result.effects_status.state === 'ready') {
        setOpen(true);
      }
    }
  }, [effectsState]);

  useEffect(() => {
    setExplainCache({ kind: 'idle' });
    setExpandedEffectId(null);
    setOpen(false);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [spec, anchor, stepPathKey]);

  function handleShowPaths(effectId: string) {
    if (expandedEffectId === effectId) {
      setExpandedEffectId(null);
      return;
    }
    setExpandedEffectId(effectId);
    if (explainCache.kind === 'idle') {
      setExplainCache({ kind: 'loading' });
      client.request({ type: 'effects_explain', subject }).then((resp) => {
        if (resp.type === 'effects_explain') {
          setExplainCache({ kind: 'ok', value: resp.result });
        } else if (resp.type === 'error') {
          setExplainCache({ kind: 'error', message: resp.message });
        }
      }).catch((e: unknown) => {
        setExplainCache({ kind: 'error', message: String(e) });
      });
    }
  }

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
    if (explainCache.kind === 'loading') {
      return <div class="effects-witnesses-loading">Loading paths…</div>;
    }
    if (explainCache.kind === 'error') {
      return <div class="effects-witnesses-error">Error: {explainCache.message}</div>;
    }
    if (explainCache.kind !== 'ok') return null;

    const explanation = explainCache.value.explanations.find((e) => e.effect_id === effectId);
    if (!explanation) return <div class="effects-no-witnesses">No witness paths found</div>;

    return (
      <div class="effects-witnesses">
        {explanation.witnesses.map((witness, wi) => (
          <div key={wi} class="effects-witness">
            {witness.hops.map((hop, hi) => (
              <div key={hi} class="effects-hop">
                <span class="mono">
                  {hop.from.spec}#{hop.from.anchor}
                  {hop.from.step_path ? ` step ${hop.from.step_path.join('.')}` : ''}
                </span>
                <span class="effects-hop-relation"> —{hop.relation}→ </span>
                <span class="mono">{hop.to.spec}#{hop.to.anchor}</span>
                {hop.site.step_text && (
                  <blockquote class="effects-hop-quote">{hop.site.step_text}</blockquote>
                )}
                {hop.boundary && (
                  <span class="effects-hop-boundary"> ({hop.boundary.execution})</span>
                )}
              </div>
            ))}
          </div>
        ))}
        {explanation.witnesses_truncated && (
          <div class="effects-witnesses-note">More paths not shown</div>
        )}
      </div>
    );
  }

  function renderBody() {
    if (effectsState.kind !== 'ok') return null;

    const { effects, issues } = effectsState.value.result;

    return (
      <>
        {effects.length > 0 ? (
          <ul class="effects-list">
            {effects.map((effect) => (
              <li key={effect.id} class="effects-item">
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
                    <span class="effects-other-sites">
                      +{effect.additional_locations} other site{effect.additional_locations !== 1 ? 's' : ''}
                    </span>
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
        {issues.length > 0 && (
          <ul class="effects-issues">
            {issues.map((issue, i) => (
              <li key={i} class="effects-issue">
                {issue.code.replace(/_/g, ' ')}
                {issue.site?.url && (
                  <> — <a href={issue.site.url} target="_blank" rel="noopener">{issue.site.url}</a></>
                )}
              </li>
            ))}
          </ul>
        )}
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
