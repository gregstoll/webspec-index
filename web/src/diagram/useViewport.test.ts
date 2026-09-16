// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
import { renderHook, act } from '@testing-library/preact';
import { useRef } from 'preact/hooks';
import { useViewport } from './useViewport';
import type { Layout } from './layout';

function makeMockSvg(width = 800, height = 600): SVGSVGElement {
  const el = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  vi.spyOn(el, 'getBoundingClientRect').mockReturnValue({
    width,
    height,
    top: 0,
    left: 0,
    right: width,
    bottom: height,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  } as DOMRect);
  return el;
}

function makeLayout(w = 400, h = 300): Layout {
  return {
    nodes: [],
    edges: [],
    width: w,
    height: h,
  };
}

describe('useViewport', () => {
  it('starts with identity transform', () => {
    const { result } = renderHook(() => {
      const ref = useRef<SVGSVGElement>(makeMockSvg());
      return useViewport(ref);
    });
    expect(result.current.transform).toEqual({ x: 0, y: 0, k: 1 });
  });

  it('fit() sets a non-identity transform', () => {
    const svg = makeMockSvg(800, 600);
    const { result } = renderHook(() => {
      const ref = useRef<SVGSVGElement>(svg);
      return useViewport(ref);
    });

    act(() => {
      result.current.fit(makeLayout(400, 300));
    });

    const t = result.current.transform;
    // Scale should be computed to fit the 400×300 layout in 800×600 container
    expect(t.k).toBeGreaterThan(0);
    expect(t.k).toBeLessThanOrEqual(4);
  });

  it('zoomBy() multiplies the scale factor', () => {
    const { result } = renderHook(() => {
      const ref = useRef<SVGSVGElement>(makeMockSvg());
      return useViewport(ref);
    });

    act(() => {
      result.current.zoomBy(2);
    });

    expect(result.current.transform.k).toBeCloseTo(2);
  });

  it('zoomBy() clamps scale to [0.2, 4]', () => {
    const { result } = renderHook(() => {
      const ref = useRef<SVGSVGElement>(makeMockSvg());
      return useViewport(ref);
    });

    act(() => {
      result.current.zoomBy(100);
    });
    expect(result.current.transform.k).toBe(4);

    act(() => {
      result.current.zoomBy(0.001);
    });
    expect(result.current.transform.k).toBe(0.2);
  });

  it('reset() returns to identity transform', () => {
    const { result } = renderHook(() => {
      const ref = useRef<SVGSVGElement>(makeMockSvg());
      return useViewport(ref);
    });

    act(() => {
      result.current.zoomBy(2);
    });
    act(() => {
      result.current.reset();
    });
    expect(result.current.transform).toEqual({ x: 0, y: 0, k: 1 });
  });

  it('two-pointer pinch apart increases scale and midpoint stays fixed within 1px', () => {
    const svg = makeMockSvg(800, 600);
    document.body.appendChild(svg);

    const { result } = renderHook(() => {
      const ref = useRef<SVGSVGElement>(svg);
      return useViewport(ref);
    });

    // getBoundingClientRect returns left=0, top=0 so client coords == container coords.
    // Place two pointers: p1 fixed at (300,300), p2 starts at (500,300).
    // Initial distance = 200.
    act(() => {
      svg.dispatchEvent(
        new PointerEvent('pointerdown', { bubbles: true, pointerId: 1, clientX: 300, clientY: 300, button: 0 })
      );
      svg.dispatchEvent(
        new PointerEvent('pointerdown', { bubbles: true, pointerId: 2, clientX: 500, clientY: 300, button: 0 })
      );
    });

    // Capture transform before the move. At this point the pinch just started;
    // prevPinchDist=200 and transform is still {0,0,1}.
    const { x: xBefore, y: yBefore, k: kBefore } = result.current.transform;

    // Move p2 to (700,300) — distance becomes 400, factor = 400/200 = 2.
    // The midpoint of the two pointers during this event is (300+700)/2 = 500, (300+300)/2 = 300.
    // We check that the world point under that midpoint is unchanged.
    act(() => {
      svg.dispatchEvent(
        new PointerEvent('pointermove', { bubbles: true, pointerId: 2, clientX: 700, clientY: 300 })
      );
    });

    const { x, y, k } = result.current.transform;

    // Scale must have increased.
    expect(k).toBeGreaterThan(kBefore);

    // The midpoint of the two pointers after the move is (300+700)/2=500, 300.
    // That same client-space point should map to the same world coordinate as before.
    const pivotX = 500;
    const pivotY = 300;
    const worldXBefore = (pivotX - xBefore) / kBefore;
    const worldYBefore = (pivotY - yBefore) / kBefore;
    const worldXAfter = (pivotX - x) / k;
    const worldYAfter = (pivotY - y) / k;

    expect(Math.abs(worldXAfter - worldXBefore)).toBeLessThan(1);
    expect(Math.abs(worldYAfter - worldYBefore)).toBeLessThan(1);

    document.body.removeChild(svg);
  });
});
