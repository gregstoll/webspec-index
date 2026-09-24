// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, screen, waitFor, cleanup, fireEvent } from '@testing-library/preact';

afterEach(cleanup);

// Mock getBoundingClientRect so Diagram's fit() has a real container size.
beforeEach(() => {
  vi.spyOn(SVGSVGElement.prototype, 'getBoundingClientRect').mockReturnValue({
    width: 800,
    height: 400,
    top: 0,
    left: 0,
    right: 800,
    bottom: 400,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  } as DOMRect);
});

import { EffectsPanel } from './EffectsPanel';
import { MockClient } from '../api/client';
import type { Request, Response } from '../api/types';

describe('EffectsPanel', () => {
  it('renders two effect labels for HTML#navigate', async () => {
    render(<EffectsPanel client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText(/may fire event name=load/)).toBeTruthy();
      expect(screen.getByText(/may queue task task=networking task/)).toBeTruthy();
    });
  });

  it('shows Partial analysis status line for HTML#navigate', async () => {
    render(<EffectsPanel client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText(/Partial analysis/)).toBeTruthy();
    });
  });

  it('clicking Show paths renders a diagram with hop target nodes', async () => {
    const client = new MockClient();
    const requests = vi.spyOn(client, 'request');
    render(<EffectsPanel client={client} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText(/may fire event name=load/)).toBeTruthy();
    });
    const showPathsBtn = screen.getAllByRole('button', { name: /Show paths/ })[0];
    fireEvent.click(showPathsBtn);
    await waitFor(() => {
      expect(requests).toHaveBeenCalledWith({
        type: 'effects_paths',
        subject: { spec: 'HTML', anchor: 'navigate' },
        effect_id: 'effect-1',
        limit: 8,
      });
      const svg = document.querySelector('svg[role="img"]');
      expect(svg).toBeTruthy();
      expect(svg!.textContent).toContain('HTML#fire-an-event');
      expect(svg!.textContent).toContain('HTML#concept-event-fire');
      expect(screen.getByText('Witness 1 · 2 hops')).toBeTruthy();
      expect(screen.getByText('All witness data')).toBeTruthy();
    });
  });

  it('shows step_text as sublabel in diagram SVG', async () => {
    render(<EffectsPanel client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText(/may fire event name=load/)).toBeTruthy();
    });
    const showPathsBtn = screen.getAllByRole('button', { name: /Show paths/ })[0];
    fireEvent.click(showPathsBtn);
    await waitFor(() => {
      const svg = document.querySelector('svg[role="img"]');
      expect(svg).toBeTruthy();
      expect(svg!.textContent).toContain('Fire an event named load at the document');
    });
  });

  it('shows subject_not_found error text for unknown anchor', async () => {
    render(<EffectsPanel client={new MockClient()} spec="HTML" anchor="browsing-the-web" />);
    await waitFor(() => {
      expect(screen.getByText('No prepared analysis for this subject')).toBeTruthy();
    });
  });

  it('tallies analysis issues per code instead of listing them all up front', async () => {
    render(<EffectsPanel client={new MockClient()} spec="HTML" anchor="navigate" />);
    const summary = await screen.findByText(/3 analysis issues/);
    expect(summary.textContent).toContain('unresolved invocation ×2');
    expect(summary.textContent).toContain('missing spec ×1');
    expect(summary.closest('details')?.open).toBe(false);
  });

  it('lets readers request more witness examples for one effect', async () => {
    class MorePathsClient extends MockClient {
      override async request(req: Request): Promise<Response> {
        const response = await super.request(req);
        if (req.type !== 'effects_paths' || response.type !== 'effects_paths') return response;
        const explanation = response.result.explanations[0];
        return {
          ...response,
          result: {
            ...response.result,
            explanations: [{
              ...explanation,
              witnesses: req.limit === 8 ? explanation.witnesses : [...explanation.witnesses, ...explanation.witnesses],
              witnesses_truncated: req.limit === 8,
            }],
          },
        };
      }
    }
    render(<EffectsPanel client={new MorePathsClient()} spec="HTML" anchor="navigate" />);
    await screen.findByText(/may fire event name=load/);
    fireEvent.click(screen.getAllByRole('button', { name: /Show paths/ })[0]);
    fireEvent.click(await screen.findByRole('button', { name: 'Load more paths' }));
    await waitFor(() => {
      expect(screen.getByText(/2 witness examples/)).toBeTruthy();
      expect(screen.queryByRole('button', { name: 'Load more paths' })).toBeNull();
    });
  });

  it('lets readers inspect analysis issues beyond the initial list', async () => {
    class ManyIssuesClient extends MockClient {
      override async request(req: Request): Promise<Response> {
        const response = await super.request(req);
        if (req.type !== 'effects' || response.type !== 'effects') return response;
        return {
          ...response,
          result: {
            ...response.result,
            issues: Array.from({ length: 31 }, (_, index) => ({
              code: 'unresolved_invocation' as const,
              message: `issue ${index + 1}`,
            })),
          },
        };
      }
    }
    render(<EffectsPanel client={new ManyIssuesClient()} spec="HTML" anchor="navigate" />);
    const summary = await screen.findByText(/31 analysis issues/);
    fireEvent.click(summary);
    expect(screen.queryByText(/issue 31/)).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Show more issues/ }));
    expect(screen.getByText(/issue 31/)).toBeTruthy();
  });

  it('shows effects of defined step bodies', async () => {
    class DefinedBodyClient extends MockClient {
      override async request(req: Request): Promise<Response> {
        const response = await super.request(req);
        if (req.type !== 'effects' || response.type !== 'effects') return response;
        return {
          ...response,
          result: {
            ...response.result,
            defined_bodies: [{
              subject: { spec: 'HTML', anchor: 'navigate', snapshot_sha: 'abc123', body_id: 'on completion' },
              effects: [{ id: 'body-effect', kind: 'fire_event', params: { name: 'complete' }, execution: ['separate'] }],
              effects_status: response.result.effects_status,
            }],
          },
        };
      }
    }
    render(<EffectsPanel client={new DefinedBodyClient()} spec="HTML" anchor="navigate" />);
    const summary = await screen.findByText('1 defined step body');
    fireEvent.click(summary);
    expect(screen.getByText(/on completion/)).toBeTruthy();
    expect(screen.getByText(/may fire event name=complete/)).toBeTruthy();
  });
});
