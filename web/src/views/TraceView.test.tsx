// @vitest-environment jsdom
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/preact';
import { TraceView } from './TraceView';
import { createTraceStore } from '../trace/store';

vi.mock('../trace/share', () => ({
  decodeTrace: vi.fn(async (payload: string) => {
    if (payload === 'bad') throw new Error('Invalid trace link: not valid JSON.');
    return [
      { spec: 'HTML', anchor: 'navigate', step_path: [3, 1] },
      { spec: 'DOM', anchor: 'concept-tree', note: 'target' },
    ];
  }),
}));

const store = createTraceStore(window.localStorage);
vi.mock('../trace/store', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../trace/store')>();
  return { ...mod, get traceStore() { return store; } };
});

afterEach(() => {
  cleanup();
  store.clear();
  location.hash = '';
});

describe('TraceView', () => {
  it('renders decoded entries with step labels and section links', async () => {
    render(<TraceView payload="ok" />);
    const link = await screen.findByRole('link', { name: /HTML#navigate/ });
    expect(link.getAttribute('href')).toBe('#/HTML/navigate?step=3.1');
    expect(link.textContent).toContain('step 3.1');
    expect(screen.getByText('target')).toBeTruthy();
  });

  it('shows the decode error in a banner', async () => {
    render(<TraceView payload="bad" />);
    await screen.findByText(/not valid JSON/);
  });

  it('imports entries once even when the button is clicked twice', async () => {
    render(<TraceView payload="ok" />);
    const btn = await screen.findByRole('button', { name: 'Import into my recorder' });
    btn.click();
    btn.click();
    await waitFor(() => expect((btn as HTMLButtonElement).disabled).toBe(true));
    expect(store.entries.map((e) => e.anchor)).toEqual(['navigate', 'concept-tree']);
    expect(location.hash).toBe('#/HTML/navigate?step=3.1');
  });
});
