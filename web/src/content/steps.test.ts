// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { annotateSteps } from './steps';

describe('annotateSteps', () => {
  it('annotates a flat OL with 1-based paths', () => {
    const root = document.createElement('div');
    root.innerHTML = '<ol><li>A</li><li>B</li><li>C</li></ol>';
    annotateSteps(root);
    const lis = Array.from(root.querySelectorAll('li'));
    expect(lis[0].dataset.stepPath).toBe('1');
    expect(lis[1].dataset.stepPath).toBe('2');
    expect(lis[2].dataset.stepPath).toBe('3');
  });

  it('annotates nested OLs with dotted paths', () => {
    const root = document.createElement('div');
    root.innerHTML = `
      <ol>
        <li>A
          <ol>
            <li>A1</li>
            <li>A2</li>
          </ol>
        </li>
        <li>B</li>
      </ol>
    `;
    annotateSteps(root);
    const lis = Array.from(root.querySelectorAll('li'));
    expect(lis[0].dataset.stepPath).toBe('1');
    expect(lis[1].dataset.stepPath).toBe('1.1');
    expect(lis[2].dataset.stepPath).toBe('1.2');
    expect(lis[3].dataset.stepPath).toBe('2');
  });

  it('handles two top-level OLs independently with separate 1-based counts', () => {
    const root = document.createElement('div');
    root.innerHTML = '<ol><li>X</li></ol><ol><li>Y</li></ol>';
    annotateSteps(root);
    const lis = Array.from(root.querySelectorAll('li'));
    expect(lis[0].dataset.stepPath).toBe('1');
    expect(lis[1].dataset.stepPath).toBe('1');
  });

  it('adds a step-select button to each li', () => {
    const root = document.createElement('div');
    root.innerHTML = '<ol><li>Step one</li></ol>';
    annotateSteps(root);
    const btn = root.querySelector('.step-select-btn') as HTMLButtonElement;
    expect(btn).toBeTruthy();
    expect(btn.tagName).toBe('BUTTON');
    expect(btn.type).toBe('button');
    expect(btn.getAttribute('aria-label')).toBe('Select step 1');
  });

  it('sets aria-label on nested step buttons using dotted path', () => {
    const root = document.createElement('div');
    root.innerHTML = '<ol><li>A<ol><li>A1</li></ol></li></ol>';
    annotateSteps(root);
    const btns = Array.from(root.querySelectorAll<HTMLButtonElement>('.step-select-btn'));
    expect(btns[0].getAttribute('aria-label')).toBe('Select step 1');
    expect(btns[1].getAttribute('aria-label')).toBe('Select step 1.1');
  });

  it('is idempotent: calling twice does not add duplicate buttons', () => {
    const root = document.createElement('div');
    root.innerHTML = '<ol><li>Step</li></ol>';
    annotateSteps(root);
    annotateSteps(root);
    expect(root.querySelectorAll('.step-select-btn').length).toBe(1);
  });

  it('does not annotate UL items', () => {
    const root = document.createElement('div');
    root.innerHTML = '<ul><li>Bullet</li></ul><ol><li>Step</li></ol>';
    annotateSteps(root);
    const ulLi = root.querySelector('ul > li') as HTMLLIElement;
    const olLi = root.querySelector('ol > li') as HTMLLIElement;
    expect(ulLi.dataset.stepPath).toBeUndefined();
    expect(olLi.dataset.stepPath).toBe('1');
  });
});
