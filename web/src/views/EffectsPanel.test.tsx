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
    render(<EffectsPanel client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText(/may fire event name=load/)).toBeTruthy();
    });
    const showPathsBtn = screen.getAllByRole('button', { name: /Show paths/ })[0];
    fireEvent.click(showPathsBtn);
    await waitFor(() => {
      const svg = document.querySelector('svg[role="img"]');
      expect(svg).toBeTruthy();
      expect(svg!.textContent).toContain('HTML#fire-an-event');
      expect(svg!.textContent).toContain('HTML#concept-event-fire');
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
});
