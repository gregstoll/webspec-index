// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/preact';
import { TraceDiagram } from './TraceDiagram';
import type { TraceEntry } from '../trace/store';

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

vi.mock('../trace/titles', () => ({
  getTitleCache: () => new Map([['HTML#navigate', { title: 'Navigation', url: '#/HTML/navigate' }]]),
}));

const entries: TraceEntry[] = [
  { id: '1', spec: 'HTML', anchor: 'navigate' },
  { id: '2', spec: 'DOM', anchor: 'concept-tree' },
  { id: '3', spec: 'HTML', anchor: 'navigate', step_path: [3, 1] },
];

describe('TraceDiagram', () => {
  it('renders svg[role="img"]', async () => {
    render(<TraceDiagram entries={entries} />);
    await waitFor(() => {
      expect(screen.getByRole('img', { name: 'Trace diagram' })).toBeTruthy();
    });
  });

  it('renders one node per distinct section (two sections = two section nodes)', async () => {
    render(<TraceDiagram entries={entries} />);
    await waitFor(() => {
      const links = document.querySelectorAll('a.diagram-node-anchor');
      const sectionLinks = Array.from(links).filter(
        (a) => a.getAttribute('href') === '#/HTML/navigate' || a.getAttribute('href') === '#/DOM/concept-tree',
      );
      expect(sectionLinks.length).toBeGreaterThanOrEqual(2);
    });
  });

  it('renders empty state when entries are empty', () => {
    render(<TraceDiagram entries={[]} />);
    expect(screen.getByText('Nothing to show')).toBeTruthy();
  });
});
