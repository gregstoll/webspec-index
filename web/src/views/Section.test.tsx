// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach } from 'vitest';
import { render, screen, waitFor, cleanup, fireEvent } from '@testing-library/preact';

afterEach(cleanup);
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

describe('Section step selection', () => {
  beforeEach(() => {
    // Reset hash before each test
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
