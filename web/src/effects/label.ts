import type { EffectSummary } from '../api/types';

function humaniseKind(kind: string): string {
  return kind.replace(/[_-]/g, ' ');
}

function executionSuffix(execution: string[]): string {
  const hasInline = execution.includes('inline');
  const hasSeparate = execution.includes('separate');
  if (hasInline && hasSeparate) return ' (inline or separate)';
  if (!hasInline) return ' (separate)';
  return '';
}

export function effectLabel(effect: EffectSummary): string {
  const kind = humaniseKind(effect.kind);

  const params = Object.entries(effect.params)
    .filter(([, v]) => v !== null)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([k, v]) => `${k}=${v}`)
    .join(' ');

  const suffix = executionSuffix(effect.execution);

  return ['may', kind, params].filter(Boolean).join(' ') + suffix;
}
