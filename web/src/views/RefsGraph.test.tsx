// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, waitFor, cleanup, fireEvent } from '@testing-library/preact';
import { RefsGraph } from './RefsGraph';
import { MockClient } from '../api/client';
import type { WebspecClient } from '../api/client';
import type { Request, Response } from '../api/types';
import type { GraphResult } from '../api/types';

afterEach(cleanup);

beforeEach(() => {
  vi.spyOn(SVGSVGElement.prototype, 'getBoundingClientRect').mockReturnValue({
    width: 800,
    height: 600,
    top: 0,
    left: 0,
    right: 800,
    bottom: 600,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  } as DOMRect);
});

/** Client that resolves the initial navigate graph immediately but defers graph requests for other targets. */
class SlowExpandClient implements WebspecClient {
  private _resolveExpand: ((r: Response) => void) | null = null;

  request(req: Request): Promise<Response> {
    if (req.type === 'graph' && req.target === 'HTML#navigate') {
      return new MockClient().request(req);
    }
    if (req.type === 'graph') {
      return new Promise<Response>((resolve) => {
        this._resolveExpand = resolve;
      });
    }
    return new MockClient().request(req);
  }

  resolveExpand() {
    this._resolveExpand?.({ type: 'graph', result: { root: { spec: 'HTML', anchor: 'X' }, direction: 'both', max_depth: 1, max_nodes: 60, nodes: [], edges: [], truncated: false } satisfies GraphResult });
  }
}

describe('RefsGraph', () => {
  it('renders an svg[role=img] after loading', async () => {
    const { container } = render(
      <RefsGraph client={new MockClient()} spec="HTML" anchor="navigate" />
    );
    await waitFor(() => {
      expect(container.querySelector('svg[role="img"]')).toBeTruthy();
    });
  });

  it('renders one diagram-node-group per graph node returned by MockClient', async () => {
    const { container } = render(
      <RefsGraph client={new MockClient()} spec="HTML" anchor="navigate" />
    );
    await waitFor(() => {
      const svg = container.querySelector('svg[role="img"]');
      expect(svg).toBeTruthy();
      // MockClient graph fixture for HTML#navigate has 5 nodes
      const nodeGroups = svg?.querySelectorAll('.diagram-node-group');
      expect(nodeGroups?.length).toBe(5);
    });
  });

  it('unmounting before expand resolves does not emit a console.error warning', async () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const client = new SlowExpandClient();

    const { container, unmount } = render(
      <RefsGraph client={client} spec="HTML" anchor="navigate" />
    );

    // Wait for initial graph to appear
    await waitFor(() => {
      expect(container.querySelector('svg[role="img"]')).toBeTruthy();
    });

    // Trigger expand by clicking the first expand button in the diagram
    const expandBtn = container.querySelector('[role="button"][aria-label^="Expand"]');
    if (expandBtn) fireEvent.click(expandBtn);

    // Unmount before the expand response arrives
    unmount();

    // Now resolve the deferred expand response
    client.resolveExpand();

    // Give microtasks a chance to settle
    await new Promise((r) => setTimeout(r, 0));

    expect(errorSpy).not.toHaveBeenCalled();
    errorSpy.mockRestore();
  });
});
