/**
 * Annotates ordered-list items in rendered spec content with data-step-path
 * attributes (1-based dotted paths like "3.1") and inserts a keyboard-accessible
 * button affordance so users can select individual steps.
 */

export function annotateSteps(root: HTMLElement): void {
  function processOl(ol: HTMLOListElement, parentPath: number[]): void {
    let index = 1;
    for (const child of Array.from(ol.children)) {
      if (child.tagName !== 'LI') continue;
      const li = child as HTMLLIElement;
      const path = [...parentPath, index];
      const pathStr = path.join('.');
      li.dataset.stepPath = pathStr;

      if (!li.querySelector('.step-select-btn')) {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'step-select-btn';
        btn.setAttribute('aria-label', `Select step ${pathStr}`);
        btn.textContent = '§';
        li.prepend(btn);
      }

      for (const nestedOl of Array.from(
        li.querySelectorAll<HTMLOListElement>(':scope > ol'),
      )) {
        processOl(nestedOl, path);
      }

      index++;
    }
  }

  for (const ol of Array.from(root.querySelectorAll<HTMLOListElement>('ol'))) {
    let el: Element | null = ol.parentElement;
    let isNested = false;
    while (el && el !== root) {
      if (el.tagName === 'OL') {
        isNested = true;
        break;
      }
      el = el.parentElement;
    }
    if (!isNested) processOl(ol, []);
  }
}
