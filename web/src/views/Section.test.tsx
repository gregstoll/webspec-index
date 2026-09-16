// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, screen, waitFor, cleanup, fireEvent } from '@testing-library/preact';

afterEach(() => {
  cleanup();
  localStorage.clear();
});
import { Section } from './Section';
import { MockClient } from '../api/client';

describe('Section', () => {
  it('renders the section title for HTML#navigate', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText('navigate')).toBeTruthy();
    });
  });

  it('renders the section type badge', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText('algorithm')).toBeTruthy();
    });
  });

  it('renders the spec chip linking to #/HTML', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      const link = screen.getByRole('link', { name: 'HTML' });
      expect(link.getAttribute('href')).toBe('#/HTML');
    });
  });

  it('renders parent navigation link', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      const link = screen.getByRole('link', { name: 'Navigation' });
      expect(link.getAttribute('href')).toBe('#/HTML/navigation');
    });
  });

  it('renders incoming references with spec grouping', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText('Incoming references')).toBeTruthy();
      const link = screen.getByRole('link', { name: 'the-a-element' });
      expect(link.getAttribute('href')).toBe('#/HTML/the-a-element');
    });
  });

  it('shows error banner for not_found (HTML#nope)', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="nope" />);
    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeTruthy();
      expect(screen.getByRole('alert').textContent).toContain('No section nope in HTML');
    });
  });

  it('shows error banner for spec_not_indexed (UNKNOWN#foo)', async () => {
    render(<Section client={new MockClient()} spec="UNKNOWN" anchor="foo" />);
    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeTruthy();
      expect(screen.getByRole('alert').textContent).toContain('UNKNOWN is not part of this index');
    });
  });

  it('post-render pass: external links get target/_blank+rel=noopener, internal links do not', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      const ext = screen.getByRole('link', { name: 'ext' });
      expect(ext.getAttribute('target')).toBe('_blank');
      expect(ext.getAttribute('rel')).toBe('noopener');

      const int = screen.getByRole('link', { name: 'int' });
      expect(int.getAttribute('target')).toBeNull();
      expect(int.getAttribute('rel')).toBeNull();
    });
  });

  it('shows effect badges under the steps they originate from', async () => {
    const { container } = render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      const step1 = container.querySelector('li[data-step-path="1"] .step-effect');
      const step2 = container.querySelector('li[data-step-path="2"] .step-effect');
      expect(step1?.textContent).toContain('may fire event name=load');
      expect(step1?.classList.contains('step-effect-path')).toBe(true);
      expect(step2?.textContent).toContain('may queue task');
      expect(step2?.classList.contains('step-effect-direct')).toBe(true);
    });
  });

  it('clicking an inline effect badge selects its step and opens that effect in the side panel', async () => {
    window.location.hash = '#/HTML/navigate';
    const { container } = render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    const badge = await waitFor(() => {
      const el = container.querySelector<HTMLButtonElement>('li[data-step-path="2"] .step-effect');
      expect(el).toBeTruthy();
      return el!;
    });
    fireEvent.click(badge);
    expect(window.location.hash).toBe('#/HTML/navigate?step=2');
    await waitFor(() => {
      const item = container.querySelector('aside.section-aside #effect-effect-2');
      expect(item?.textContent).toContain('Hide paths');
    });
  });

  it('post-render pass: javascript: links lose their href', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByRole('link', { name: 'ext' })).toBeTruthy();
      const js = screen.getByText('js');
      expect(js.tagName).toBe('A');
      expect(js.hasAttribute('href')).toBe(false);
    });
  });
});

describe('Section refs graph toggle', () => {
  beforeEach(() => {
    vi.spyOn(SVGSVGElement.prototype, 'getBoundingClientRect').mockReturnValue({
      width: 800, height: 600, top: 0, left: 0, right: 800, bottom: 600, x: 0, y: 0,
      toJSON: () => ({}),
    } as DOMRect);
  });

  it('clicking "Graph" mounts an svg[role="img"]', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText('navigate')).toBeTruthy();
    });
    const toggleBtn = screen.getByRole('button', { name: 'Graph' });
    fireEvent.click(toggleBtn);
    await waitFor(() => {
      expect(document.querySelector('svg[role="img"]')).toBeTruthy();
    });
  });
});

describe('Section flow toggle', () => {
  beforeEach(() => {
    vi.spyOn(SVGSVGElement.prototype, 'getBoundingClientRect').mockReturnValue({
      width: 800, height: 600, top: 0, left: 0, right: 800, bottom: 600, x: 0, y: 0,
      toJSON: () => ({}),
    } as DOMRect);
    window.location.hash = '#/HTML/navigate';
  });

  it('switching to Flow renders svg[role="img"]', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText('navigate')).toBeTruthy();
    });
    const flowBtn = screen.getByRole('button', { name: 'Flow' });
    fireEvent.click(flowBtn);
    await waitFor(() => {
      expect(document.querySelector('svg[role="img"]')).toBeTruthy();
    });
  });

  it('clicking a step node in Flow mode sets location.hash to #/HTML/navigate?step=1', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(screen.getByText('navigate')).toBeTruthy();
    });
    fireEvent.click(screen.getByRole('button', { name: 'Flow' }));
    await waitFor(() => {
      expect(document.querySelector('svg[role="img"]')).toBeTruthy();
    });
    // Step node "1" has no href, so it renders as a role=button g element
    const stepNode = document.querySelector<SVGElement>('g[role="button"][aria-label]');
    expect(stepNode).toBeTruthy();
    fireEvent.click(stepNode!);
    expect(window.location.hash).toBe('#/HTML/navigate?step=1');
  });

  it('rendering with selectedStepPath=[1] in Flow mode marks step node 1 as selected', async () => {
    localStorage.setItem('sectionView', 'flow');
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" selectedStepPath={[1]} />);
    await waitFor(() => {
      expect(document.querySelector('svg[role="img"]')).toBeTruthy();
    });
    await waitFor(() => {
      const selected = document.querySelector('.diagram-node.is-selected');
      expect(selected).toBeTruthy();
    });
  });
});

describe('Section step selection', () => {
  beforeEach(() => {
    window.location.hash = '#/HTML/navigate';
  });

  it('clicking a step-select-btn sets location.hash to #/HTML/navigate?step=1', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" />);
    await waitFor(() => {
      expect(document.querySelector('.step-select-btn')).toBeTruthy();
    });
    const btns = document.querySelectorAll<HTMLButtonElement>('.step-select-btn');
    fireEvent.click(btns[0]);
    expect(window.location.hash).toBe('#/HTML/navigate?step=1');
  });

  it('rendering with selectedStepPath=[1] marks the first step li with aria-current="true"', async () => {
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" selectedStepPath={[1]} />);
    await waitFor(() => {
      const li = document.querySelector<HTMLLIElement>('li[data-step-path="1"]');
      expect(li).toBeTruthy();
      expect(li!.getAttribute('aria-current')).toBe('true');
    });
  });

  it('clicking the already-selected step button clears ?step from the hash', async () => {
    window.location.hash = '#/HTML/navigate?step=1';
    render(<Section client={new MockClient()} spec="HTML" anchor="navigate" selectedStepPath={[1]} />);
    await waitFor(() => {
      expect(document.querySelector('li[aria-current="true"]')).toBeTruthy();
    });
    const btn = document.querySelector<HTMLButtonElement>('li[data-step-path="1"] .step-select-btn');
    expect(btn).toBeTruthy();
    fireEvent.click(btn!);
    expect(window.location.hash).toBe('#/HTML/navigate');
  });
});
