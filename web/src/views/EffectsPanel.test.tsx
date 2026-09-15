// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup, fireEvent } from '@testing-library/preact';

afterEach(cleanup);

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

  it('clicking Show paths renders hop lines for the first effect', async () => {
    render(<EffectsPanel client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText(/may fire event name=load/)).toBeTruthy();
    });
    const showPathsBtn = screen.getAllByRole('button', { name: /Show paths/ })[0];
    fireEvent.click(showPathsBtn);
    await waitFor(() => {
      expect(screen.getAllByText(/—invoke→/).length).toBeGreaterThan(0);
      expect(screen.getByText(/HTML#concept-event-fire/)).toBeTruthy();
    });
  });

  it('shows step_text quote in witness hops', async () => {
    render(<EffectsPanel client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText(/may fire event name=load/)).toBeTruthy();
    });
    const showPathsBtn = screen.getAllByRole('button', { name: /Show paths/ })[0];
    fireEvent.click(showPathsBtn);
    await waitFor(() => {
      expect(screen.getByText('Fire an event named load at the document')).toBeTruthy();
    });
  });

  it('shows subject_not_found error text for unknown anchor', async () => {
    render(<EffectsPanel client={new MockClient()} spec="HTML" anchor="browsing-the-web" />);
    await waitFor(() => {
      expect(screen.getByText('No prepared analysis for this subject')).toBeTruthy();
    });
  });
});
