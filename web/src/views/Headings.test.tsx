// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup } from '@testing-library/preact';

afterEach(cleanup);
import { Headings } from './Headings';
import { MockClient } from '../api/client';

describe('Headings', () => {
  it('renders heading titles', async () => {
    render(<Headings client={new MockClient()} spec="HTML" />);
    await waitFor(() => {
      expect(screen.getByText('Browsing the web')).toBeTruthy();
    });
  });

  it('renders section-number span when number is present', async () => {
    render(<Headings client={new MockClient()} spec="HTML" />);
    await waitFor(() => {
      const spans = document.querySelectorAll('.section-number');
      expect(spans.length).toBeGreaterThan(0);
    });
  });

  it('shows the number text for a numbered heading', async () => {
    render(<Headings client={new MockClient()} spec="HTML" />);
    await waitFor(() => {
      expect(screen.getByText('7.4')).toBeTruthy();
    });
  });

  it('does not render section-number span when number is absent', async () => {
    render(<Headings client={new MockClient()} spec="HTML" />);
    await waitFor(() => {
      // navigate has no number in fixtures; only numbered spans should be present
      const spans = document.querySelectorAll('.section-number');
      // Confirm they don't contain empty text
      for (const span of spans) {
        expect((span.textContent ?? '').trim()).not.toBe('');
      }
    });
  });
});
