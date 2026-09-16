import { useState, useCallback, useRef, useEffect } from 'preact/hooks';
import type { RefObject } from 'preact';
import type { Layout } from './layout';

export interface Transform {
  x: number;
  y: number;
  k: number;
}

const K_MIN = 0.2;
const K_MAX = 4;

function clampK(k: number): number {
  return Math.min(K_MAX, Math.max(K_MIN, k));
}

function dist(a: { x: number; y: number }, b: { x: number; y: number }): number {
  return Math.hypot(b.x - a.x, b.y - a.y);
}

export interface Viewport {
  transform: Transform;
  fit: (layout: Layout) => void;
  /** Set scale to 1 and scroll so the topmost node is visible at the top-left with a small margin. */
  top: (layout: Layout) => void;
  zoomBy: (factor: number, center?: { x: number; y: number }) => void;
  reset: () => void;
}

/**
 * Manages pan/zoom transform for a diagram SVG element and wires pointer
 * events to the element referenced by `containerRef`.
 *
 * - Primary-button drag pans the canvas.
 * - Wheel zooms around the cursor, k clamped to [0.2, 4].
 * - Two-pointer pinch zooms around the midpoint of the two pointers using the
 *   ratio of current to previous distance; k clamped to [0.2, 4].
 * - Active pointers are tracked in a Map keyed by pointerId; a pointer is
 *   removed on pointerup or pointercancel.
 */
export function useViewport(containerRef: RefObject<SVGSVGElement>): Viewport {
  const [transform, setTransform] = useState<Transform>({ x: 0, y: 0, k: 1 });

  // Ref mirror so event handlers always read the latest transform.
  const transformRef = useRef<Transform>(transform);
  transformRef.current = transform;

  // Single-pointer drag state.
  const dragRef = useRef<{ startX: number; startY: number; tx: number; ty: number } | null>(null);

  // Active pointer positions keyed by pointerId.
  const pointersRef = useRef<Map<number, { x: number; y: number }>>(new Map());
  // Previous pinch distance (set when two pointers are active).
  const prevPinchDistRef = useRef<number | null>(null);

  const fit = useCallback(
    (layout: Layout) => {
      const el = containerRef.current;
      let cw = 800;
      let ch = 600;
      if (el) {
        const rect = el.getBoundingClientRect();
        if (rect.width !== 0 || rect.height !== 0) {
          cw = rect.width;
          ch = rect.height;
        }
      }
      if (layout.width === 0 || layout.height === 0) {
        setTransform({ x: 0, y: 0, k: 1 });
        return;
      }
      const k = clampK(Math.min(cw / layout.width, ch / layout.height) * 0.9);
      const x = (cw - layout.width * k) / 2;
      const y = (ch - layout.height * k) / 2;
      setTransform({ x, y, k });
    },
    [containerRef]
  );

  const top = useCallback((layout: Layout) => {
    const MARGIN = 16;
    if (layout.nodes.length === 0) {
      setTransform({ x: MARGIN, y: MARGIN, k: 1 });
      return;
    }
    // Find the topmost (smallest top edge) and leftmost node.
    const minY = Math.min(...layout.nodes.map((n) => n.y - n.height / 2));
    const minX = Math.min(...layout.nodes.map((n) => n.x - n.width / 2));
    setTransform({ x: MARGIN - minX, y: MARGIN - minY, k: 1 });
  }, []);

  const zoomBy = useCallback(
    (factor: number, center?: { x: number; y: number }) => {
      setTransform((prev) => {
        const newK = clampK(prev.k * factor);
        if (center == null) {
          return { ...prev, k: newK };
        }
        const x = center.x - (center.x - prev.x) * (newK / prev.k);
        const y = center.y - (center.y - prev.y) * (newK / prev.k);
        return { x, y, k: newK };
      });
    },
    []
  );

  const reset = useCallback(() => {
    setTransform({ x: 0, y: 0, k: 1 });
  }, []);

  useEffect(() => {
    const el: SVGSVGElement | null = containerRef.current;
    if (el == null) return;
    const svg: SVGSVGElement = el;

    function onPointerDown(e: PointerEvent) {
      pointersRef.current.set(e.pointerId, { x: e.clientX, y: e.clientY });

      if (pointersRef.current.size === 1 && e.button === 0) {
        // Start single-pointer drag.
        const t = transformRef.current;
        dragRef.current = { startX: e.clientX, startY: e.clientY, tx: t.x, ty: t.y };
        if (svg.setPointerCapture) {
          svg.setPointerCapture(e.pointerId);
        }
      } else if (pointersRef.current.size === 2) {
        // A second pointer arrived — cancel drag, start pinch.
        dragRef.current = null;
        const pts = [...pointersRef.current.values()];
        prevPinchDistRef.current = dist(pts[0], pts[1]);
      }
    }

    function onPointerMove(e: PointerEvent) {
      pointersRef.current.set(e.pointerId, { x: e.clientX, y: e.clientY });

      if (pointersRef.current.size >= 2) {
        // Pinch zoom.
        const pts = [...pointersRef.current.values()];
        const currentDist = dist(pts[0], pts[1]);
        const prevDist = prevPinchDistRef.current;
        if (prevDist != null && prevDist > 0) {
          const factor = currentDist / prevDist;
          const mx = (pts[0].x + pts[1].x) / 2;
          const my = (pts[0].y + pts[1].y) / 2;
          const rect = svg.getBoundingClientRect();
          const cx = mx - rect.left;
          const cy = my - rect.top;
          setTransform((prev) => {
            const newK = clampK(prev.k * factor);
            const x = cx - (cx - prev.x) * (newK / prev.k);
            const y = cy - (cy - prev.y) * (newK / prev.k);
            return { x, y, k: newK };
          });
        }
        prevPinchDistRef.current = currentDist;
        return;
      }

      // Single-pointer drag.
      if (dragRef.current == null) return;
      const { startX, startY, tx, ty } = dragRef.current;
      const dx = e.clientX - startX;
      const dy = e.clientY - startY;
      setTransform((prev) => ({ ...prev, x: tx + dx, y: ty + dy }));
    }

    function onPointerUp(e: PointerEvent) {
      pointersRef.current.delete(e.pointerId);
      if (pointersRef.current.size < 2) {
        prevPinchDistRef.current = null;
      }
      if (pointersRef.current.size === 0) {
        dragRef.current = null;
      }
    }

    function onWheel(e: WheelEvent) {
      e.preventDefault();
      const rect = svg.getBoundingClientRect();
      const cx = e.clientX - rect.left;
      const cy = e.clientY - rect.top;
      const factor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
      setTransform((prev) => {
        const newK = clampK(prev.k * factor);
        const x = cx - (cx - prev.x) * (newK / prev.k);
        const y = cy - (cy - prev.y) * (newK / prev.k);
        return { x, y, k: newK };
      });
    }

    svg.addEventListener('pointerdown', onPointerDown);
    svg.addEventListener('pointermove', onPointerMove);
    svg.addEventListener('pointerup', onPointerUp);
    svg.addEventListener('pointercancel', onPointerUp);
    svg.addEventListener('wheel', onWheel, { passive: false });

    return () => {
      svg.removeEventListener('pointerdown', onPointerDown);
      svg.removeEventListener('pointermove', onPointerMove);
      svg.removeEventListener('pointerup', onPointerUp);
      svg.removeEventListener('pointercancel', onPointerUp);
      svg.removeEventListener('wheel', onWheel);
    };
  }, [containerRef]);

  return { transform, fit, top, zoomBy, reset };
}
