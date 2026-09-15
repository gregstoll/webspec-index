// @vitest-environment jsdom
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/preact';
import { TracePanel } from './TracePanel';
import type { Route } from '../router';

afterEach(cleanup);

const landingRoute: Route = { kind: 'landing' };

describe('TracePanel', () => {
  it('renders a close button that calls onClose', async () => {
    const onClose = vi.fn();
    render(<TracePanel route={landingRoute} onClose={onClose} />);
    const btn = screen.getByRole('button', { name: 'Close trace panel' });
    btn.click();
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
