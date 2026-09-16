// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, screen, waitFor, cleanup, fireEvent } from '@testing-library/preact';
import { FlowView } from './FlowView';
import { MockClient, MOCK_FLOW_NAVIGATE } from '../api/client';

afterEach(cleanup);

beforeEach(() => {
  vi.spyOn(SVGSVGElement.prototype, 'getBoundingClientRect').mockReturnValue({
    width: 800, height: 600, top: 0, left: 0, right: 800, bottom: 600, x: 0, y: 0,
    toJSON: () => ({}),
  } as DOMRect);
});

const nonExternalCount = MOCK_FLOW_NAVIGATE.nodes.filter((n) => n.kind !== 'external').length;

describe('FlowView', () => {
  it('renders an svg[role="img"] after loading', async () => {
    const { container } = render(
      <FlowView client={new MockClient()} spec="HTML" anchor="navigate" />
    );
    await waitFor(() => {
      expect(container.querySelector('svg[role="img"]')).toBeTruthy();
    });
  });

  it('renders one diagram-node-group per non-external FlowNode by default', async () => {
    const { container } = render(
      <FlowView client={new MockClient()} spec="HTML" anchor="navigate" />
    );
    await waitFor(() => {
      const svg = container.querySelector('svg[role="img"]');
      expect(svg).toBeTruthy();
      const nodeGroups = svg?.querySelectorAll('.diagram-node-group');
      expect(nodeGroups?.length).toBe(nonExternalCount);
    });
  });

  it('shows an error banner for a not_found target', async () => {
    const { container } = render(
      <FlowView client={new MockClient()} spec="HTML" anchor="unknown-algo" />
    );
    await waitFor(() => {
      expect(container.querySelector('[role="alert"]')).toBeTruthy();
    });
  });

  it('renders "Show calls as nodes" toggle button with aria-pressed=false by default', async () => {
    render(<FlowView client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      const btn = screen.getByRole('button', { name: 'Show calls as nodes' });
      expect(btn).toBeTruthy();
      expect(btn.getAttribute('aria-pressed')).toBe('false');
    });
  });

  it('toggling "Show calls as nodes" adds external nodes to the diagram', async () => {
    const { container } = render(
      <FlowView client={new MockClient()} spec="HTML" anchor="navigate" />
    );

    // Wait for initial render
    await waitFor(() => {
      expect(container.querySelector('svg[role="img"]')).toBeTruthy();
      expect(container.querySelector('.diagram-loading')).toBeFalsy();
    });

    // Toggle on
    const btn = screen.getByRole('button', { name: 'Show calls as nodes' });
    fireEvent.click(btn);

    // Now aria-pressed should be true and all nodes including external should appear
    await waitFor(() => {
      expect(btn.getAttribute('aria-pressed')).toBe('true');
      const svg = container.querySelector('svg[role="img"]');
      const nodeGroups = svg?.querySelectorAll('.diagram-node-group');
      expect(nodeGroups?.length).toBe(MOCK_FLOW_NAVIGATE.nodes.length);
    });
  });
});
