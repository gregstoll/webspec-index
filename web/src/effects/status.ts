import type { EffectsStatus, IssueCode } from '../api/types';

export interface StatusDescription {
  headline: string;
  detail?: string;
  tone: 'ok' | 'warn' | 'muted';
}

// What each analysis gap means for the reader, in the order the engine reports them.
const ISSUE_TEXT: Record<string, string> = {
  missing_spec: 'some called specs are not indexed',
  missing_anchor: 'some links point at anchors that were not found',
  unsupported_structure: 'some called algorithms have no parseable steps',
  unresolved_invocation: 'some links could not be resolved to an algorithm',
  unresolved_body_binding: 'some "run these steps" bodies could not be bound',
  unsupported_preview: 'PR previews are not analysed',
};

function humaniseCode(code: IssueCode): string {
  return ISSUE_TEXT[code] ?? code.replace(/_/g, ' ');
}

function omittedSuffix(omitted: number): string | undefined {
  return omitted > 0 ? `${omitted} effects omitted` : undefined;
}

function buildDetail(parts: string[]): string | undefined {
  return parts.length > 0 ? parts.join('; ') : undefined;
}

export function describeStatus(status: EffectsStatus): StatusDescription {
  switch (status.state) {
    case 'ready': {
      const omitted = omittedSuffix(status.omitted);
      if (status.coverage === 'complete') {
        return { headline: 'Analysis complete', tone: 'ok', detail: buildDetail(omitted ? [omitted] : []) };
      }
      const parts = status.issues.map(humaniseCode);
      if (omitted) parts.push(omitted);
      return {
        headline: 'Partial analysis',
        tone: 'warn',
        detail: buildDetail(parts.length > 0 ? [`effects listed here are a lower bound: ${parts.join('; ')}`] : []),
      };
    }
    case 'pending':
      return { headline: 'Analysis not prepared yet', tone: 'muted' };
    case 'unavailable':
      return { headline: 'No prepared analysis for this section', tone: 'muted' };
    case 'error':
      return { headline: 'Analysis error', tone: 'warn' };
    case 'disabled':
      return { headline: 'Effects not enabled', tone: 'muted' };
  }
}
