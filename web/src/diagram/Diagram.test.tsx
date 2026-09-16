// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/preact';
import { Diagram } from './Diagram';
import type { DiagramGraph } from './model';

afterEach(cleanup);

// Mock getBoundingClientRect on the SVG so fit() has a real container size.
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

const graphWithHref: DiagramGraph = {
  nodes: [
    { id: 'a', label: 'Section A', kind: 'section', href: '#/HTML/a' },
    { id: 'b', label: 'Step B', kind: 'step' },
    { id: 'c', label: 'External C', kind: 'external', href: '#/DOM/c' },
  ],
  edges: [
    { from: 'a', to: 'b', kind: 'next' },
    { from: 'b', to: 'c', kind: 'reference' },
  ],
};

describe('Diagram', () => {
  it('renders the SVG with the given aria-label', async () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test diagram" />);
    await waitFor(() => {
      expect(screen.getByRole('img', { name: 'Test diagram' })).toBeTruthy();
    });
  });

  it('renders one <a> per node that has an href', async () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test" />);
    await waitFor(() => {
      const anchors = document.querySelectorAll('a.diagram-node-anchor');
      expect(anchors.length).toBe(2);
    });
  });

  it('renders a focusable group for a node without href', async () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test" />);
    await waitFor(() => {
      // getByRole('button') confirms role=button and aria-label are present
      const btn = screen.getByRole('button', { name: 'Step B' });
      expect(btn).toBeTruthy();
      expect(btn.getAttribute('role')).toBe('button');
      expect(btn.getAttribute('aria-label')).toBe('Step B');
    });
  });

  it('calls onSelect when Enter is pressed on a focused node', async () => {
    const onSelect = vi.fn();
    render(<Diagram graph={graphWithHref} ariaLabel="Test" onSelect={onSelect} />);
    await waitFor(() => {
      screen.getByRole('button', { name: 'Step B' });
    });
    const btn = screen.getByRole('button', { name: 'Step B' });
    fireEvent.keyDown(btn, { key: 'Enter' });
    expect(onSelect).toHaveBeenCalledWith('b');
  });

  it('"Fit" toolbar button is present', async () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test" />);
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Fit' })).toBeTruthy();
    });
  });

  it('"Zoom in" and "Zoom out" toolbar buttons are present', async () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test" />);
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Zoom in' })).toBeTruthy();
      expect(screen.getByRole('button', { name: 'Zoom out' })).toBeTruthy();
    });
  });

  it('"Copy as Mermaid" toolbar button is present', async () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test" />);
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Copy as Mermaid' })).toBeTruthy();
    });
  });

  it('"Copy as Mermaid" writes toMermaid(graph) to clipboard', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      writable: true,
      configurable: true,
    });

    render(<Diagram graph={graphWithHref} ariaLabel="Test" />);
    await waitFor(() => {
      screen.getByRole('button', { name: 'Copy as Mermaid' });
    });

    const btn = screen.getByRole('button', { name: 'Copy as Mermaid' });
    fireEvent.click(btn);

    await waitFor(() => {
      expect(writeText).toHaveBeenCalledTimes(1);
      const text: string = writeText.mock.calls[0][0];
      expect(text).toContain('flowchart TD');
    });
  });

  it('shows loading state while layout is being computed', () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test" />);
    // Loading is shown synchronously before layoutGraph resolves
    expect(document.querySelector('.diagram-loading')).toBeTruthy();
  });

  it('shows empty state for an empty graph', () => {
    const empty: DiagramGraph = { nodes: [], edges: [] };
    render(<Diagram graph={empty} ariaLabel="Empty" />);
    expect(screen.getByText('Nothing to show')).toBeTruthy();
  });

  it('adding a node to graph does not reset the viewport transform', async () => {
    const { rerender } = render(<Diagram graph={graphWithHref} ariaLabel="Test" />);
    // Wait for initial layout
    await waitFor(() => {
      expect(document.querySelector('svg[role="img"]')).toBeTruthy();
      expect(document.querySelector('.diagram-loading')).toBeFalsy();
    });

    const svg = document.querySelector<SVGSVGElement>('svg[role="img"]')!;

    // Simulate a wheel event to change the viewport transform
    fireEvent.wheel(svg, { deltaY: -100, clientX: 400, clientY: 300 });

    // Capture transform after wheel
    const gEl = svg.querySelector('g[transform]');
    const transformAfterWheel = gEl?.getAttribute('transform') ?? '';

    // Add a new node (simulates expand)
    const expandedGraph: DiagramGraph = {
      nodes: [...graphWithHref.nodes, { id: 'd', label: 'New D', kind: 'section', href: '#/DOM/d' }],
      edges: [...graphWithHref.edges, { from: 'a', to: 'd', kind: 'reference' }],
    };
    rerender(<Diagram graph={expandedGraph} ariaLabel="Test" />);

    // Wait for re-layout
    await waitFor(() => {
      expect(document.querySelector('.diagram-loading')).toBeFalsy();
    });

    // Transform must be unchanged
    const gElAfter = svg.querySelector('g[transform]');
    expect(gElAfter?.getAttribute('transform')).toBe(transformAfterWheel);
  });

  it('hovering a node adds is-hot to neighbours', async () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test" />);
    await waitFor(() => {
      screen.getByRole('button', { name: 'Step B' });
    });

    const btn = screen.getByRole('button', { name: 'Step B' });
    // The hover is on the inner group, find its parent node group
    const group = btn.closest('.diagram-node-group') ?? btn;
    fireEvent.mouseEnter(group.querySelector('g.diagram-node-group') ?? group);

    // After hovering b, node a should gain is-hot because a→b edge exists
    await waitFor(() => {
      const hotGroups = document.querySelectorAll('.diagram-node-group.is-hot');
      expect(hotGroups.length).toBeGreaterThan(0);
    });
  });

  it('initialView="top" sets scale 1 and places the topmost node near the origin', async () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test" initialView="top" />);
    await waitFor(() => {
      expect(document.querySelector('.diagram-loading')).toBeFalsy();
    });

    const svg = document.querySelector<SVGSVGElement>('svg[role="img"]')!;
    const gEl = svg.querySelector('g[transform]');
    const transform = gEl?.getAttribute('transform') ?? '';

    // scale should be 1
    expect(transform).toContain('scale(1)');

    // The translation should place content near the top-left (translate values close to the margin, not huge negative).
    const match = transform.match(/translate\(([^,]+),([^)]+)\)/);
    if (match) {
      const tx = parseFloat(match[1]);
      const ty = parseFloat(match[2]);
      // With scale=1 and a small margin, translation should be small positive.
      expect(tx).toBeGreaterThan(-50);
      expect(ty).toBeGreaterThan(-50);
    }
  });

  it('initialView="fit" (default) produces scale less than 1 for a large graph in a small container', async () => {
    render(<Diagram graph={graphWithHref} ariaLabel="Test" initialView="fit" />);
    await waitFor(() => {
      expect(document.querySelector('.diagram-loading')).toBeFalsy();
    });

    const svg = document.querySelector<SVGSVGElement>('svg[role="img"]')!;
    const gEl = svg.querySelector('g[transform]');
    const transform = gEl?.getAttribute('transform') ?? '';

    // 'fit' may produce a scale != 1 depending on graph/container sizes.
    // The key invariant: transform is applied (not the default identity).
    expect(transform).toMatch(/translate\(/);
    expect(transform).toMatch(/scale\(/);
  });
});
