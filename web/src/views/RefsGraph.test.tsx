// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, waitFor, cleanup } from '@testing-library/preact';
import { RefsGraph } from './RefsGraph';
import { MockClient } from '../api/client';

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
});
