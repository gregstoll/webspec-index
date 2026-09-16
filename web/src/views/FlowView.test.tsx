// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, waitFor, cleanup } from '@testing-library/preact';
import { FlowView } from './FlowView';
import { MockClient, MOCK_FLOW_NAVIGATE } from '../api/client';

afterEach(cleanup);

beforeEach(() => {
  vi.spyOn(SVGSVGElement.prototype, 'getBoundingClientRect').mockReturnValue({
    width: 800, height: 600, top: 0, left: 0, right: 800, bottom: 600, x: 0, y: 0,
    toJSON: () => ({}),
  } as DOMRect);
});

describe('FlowView', () => {
  it('renders an svg[role="img"] after loading', async () => {
    const { container } = render(
      <FlowView client={new MockClient()} spec="HTML" anchor="navigate" />
    );
    await waitFor(() => {
      expect(container.querySelector('svg[role="img"]')).toBeTruthy();
    });
  });

  it('renders one diagram-node-group per FlowNode returned by MockClient', async () => {
    const { container } = render(
      <FlowView client={new MockClient()} spec="HTML" anchor="navigate" />
    );
    await waitFor(() => {
      const svg = container.querySelector('svg[role="img"]');
      expect(svg).toBeTruthy();
      const nodeGroups = svg?.querySelectorAll('.diagram-node-group');
      expect(nodeGroups?.length).toBe(MOCK_FLOW_NAVIGATE.nodes.length);
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
});
