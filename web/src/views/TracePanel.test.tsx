// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/preact';
import { TracePanel } from './TracePanel';
import { createTraceStore } from '../trace/store';
import type { Route } from '../router';

afterEach(cleanup);

const landingRoute: Route = { kind: 'landing' };

const store = createTraceStore(window.localStorage);
vi.mock('../trace/store', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../trace/store')>();
  return { ...mod, get traceStore() { return store; } };
});

vi.mock('../trace/titles', () => ({
  getTitleCache: () => new Map(),
}));

beforeEach(() => {
  store.clear();
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

describe('TracePanel', () => {
  it('renders a close button that calls onClose', async () => {
    const onClose = vi.fn();
    render(<TracePanel route={landingRoute} onClose={onClose} />);
    const btn = screen.getByRole('button', { name: 'Close trace panel' });
    btn.click();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('does not show Diagram button when trace is empty', () => {
    render(<TracePanel route={landingRoute} onClose={() => {}} />);
    expect(screen.queryByRole('button', { name: 'Diagram' })).toBeNull();
  });

  it('shows Diagram button when trace has entries', async () => {
    store.add({ spec: 'HTML', anchor: 'navigate' });
    render(<TracePanel route={landingRoute} onClose={() => {}} />);
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Diagram' })).toBeTruthy();
    });
  });

  it('toggles diagram on and off when Diagram button is clicked', async () => {
    store.add({ spec: 'HTML', anchor: 'navigate' });
    store.add({ spec: 'DOM', anchor: 'concept-tree' });
    render(<TracePanel route={landingRoute} onClose={() => {}} />);

    const diagramBtn = await screen.findByRole('button', { name: 'Diagram' });
    expect(screen.queryByRole('img', { name: 'Trace diagram' })).toBeNull();

    diagramBtn.click();

    await waitFor(() => {
      expect(screen.getByRole('img', { name: 'Trace diagram' })).toBeTruthy();
    });

    diagramBtn.click();

    await waitFor(() => {
      expect(screen.queryByRole('img', { name: 'Trace diagram' })).toBeNull();
    });
  });
});
