import { useEffect, useRef, useState, useId, useCallback } from 'preact/hooks';
import type { DiagramGraph } from './model';
import { layoutGraph } from './layout';
import type { Layout, LayoutNode, LayoutEdge } from './layout';
import { toMermaid } from './mermaid';
import { useViewport } from './useViewport';
import '../styles/diagram.css';

export interface DiagramProps {
  graph: DiagramGraph;
  rankdir?: 'TB' | 'LR';
  selectedId?: string;
  onSelect?: (id: string) => void;
  onExpand?: (id: string) => void;
  ariaLabel: string;
}

// ── Shapes ────────────────────────────────────────────────────────────────────

function NodeShape({
  node,
  selected,
}: {
  node: LayoutNode;
  selected: boolean;
}) {
  const hw = node.width / 2;
  const hh = node.height / 2;
  const cls = `diagram-node diagram-node--${node.kind}${selected ? ' is-selected' : ''}`;

  switch (node.kind) {
    case 'branch':
      // Diamond
      return (
        <polygon
          class={cls}
          points={`0,${-hh} ${hw},0 0,${hh} ${-hw},0`}
        />
      );
    case 'terminal':
      // Capsule (stadium / pill)
      return (
        <rect
          class={cls}
          x={-hw}
          y={-hh}
          width={node.width}
          height={node.height}
          rx={hh}
          ry={hh}
        />
      );
    case 'external':
      // Dashed border rectangle
      return (
        <rect
          class={cls}
          x={-hw}
          y={-hh}
          width={node.width}
          height={node.height}
          rx={4}
        />
      );
    case 'loop':
      // Rounded rectangle
      return (
        <rect
          class={cls}
          x={-hw}
          y={-hh}
          width={node.width}
          height={node.height}
          rx={12}
        />
      );
    case 'parallel': {
      // Rectangle with double top/bottom bars
      return (
        <g class={cls}>
          <rect x={-hw} y={-hh} width={node.width} height={node.height} rx={4} />
          <line x1={-hw + 4} y1={-hh + 4} x2={hw - 4} y2={-hh + 4} class="diagram-parallel-bar" />
          <line x1={-hw + 4} y1={hh - 4} x2={hw - 4} y2={hh - 4} class="diagram-parallel-bar" />
        </g>
      );
    }
    case 'effect':
      // Pentagon / flag shape (right-pointing)
      return (
        <polygon
          class={cls}
          points={`${-hw},${-hh} ${hw - 8},${-hh} ${hw},0 ${hw - 8},${hh} ${-hw},${hh}`}
        />
      );
    default:
      // section, step → rectangle
      return (
        <rect
          class={cls}
          x={-hw}
          y={-hh}
          width={node.width}
          height={node.height}
          rx={4}
        />
      );
  }
}

// ── Label lines ───────────────────────────────────────────────────────────────

function LabelLines({ label, sublabel }: { label: string; sublabel?: string }) {
  const WRAP = 32;
  const LINE_H = 18;

  function wrap(text: string): string[] {
    const words = text.split(' ');
    const lines: string[] = [];
    let current = '';
    for (const word of words) {
      if (current === '') {
        current = word;
      } else if (current.length + 1 + word.length <= WRAP) {
        current += ' ' + word;
      } else {
        lines.push(current);
        current = word;
      }
    }
    if (current !== '') lines.push(current);
    return lines.length > 0 ? lines : [''];
  }

  const mainLines = wrap(label);
  const totalLines = mainLines.length + (sublabel ? 1 : 0);
  const startY = -(totalLines * LINE_H) / 2 + LINE_H / 2;

  return (
    <text class="diagram-label" text-anchor="middle" dominant-baseline="middle">
      {mainLines.map((line, i) => (
        <tspan
          key={i}
          x={0}
          dy={i === 0 ? startY : LINE_H}
        >
          {line}
        </tspan>
      ))}
      {sublabel && (
        <tspan
          class="diagram-sublabel"
          x={0}
          dy={mainLines.length === 0 ? startY : LINE_H}
        >
          {sublabel}
        </tspan>
      )}
    </text>
  );
}

// ── Edge path ─────────────────────────────────────────────────────────────────

function EdgePath({
  edge,
  markerId,
  hot,
}: {
  edge: LayoutEdge;
  markerId: string;
  hot: boolean;
}) {
  const dashed = edge.kind === 'call' || edge.kind === 'unknown';
  const dotted = edge.kind === 'reference';
  const cls = [
    'diagram-edge',
    `diagram-edge--${edge.kind}`,
    hot ? 'is-hot' : '',
  ]
    .filter(Boolean)
    .join(' ');

  const pts = edge.points;
  const d =
    pts.length < 2
      ? ''
      : 'M' +
        pts
          .map((p: { x: number; y: number }, i: number) =>
            i === 0 ? `${p.x},${p.y}` : `L${p.x},${p.y}`
          )
          .join(' ');

  return (
    <path
      class={cls}
      d={d}
      stroke-dasharray={dashed ? '6 3' : dotted ? '2 4' : undefined}
      marker-end={`url(#${markerId})`}
      fill="none"
    />
  );
}

// ── Node wrapper ──────────────────────────────────────────────────────────────

function NodeGroup({
  node,
  selected,
  hot,
  onSelect,
  onExpand,
  onHoverIn,
  onHoverOut,
}: {
  node: LayoutNode;
  selected: boolean;
  hot: boolean;
  onSelect?: (id: string) => void;
  onExpand?: (id: string) => void;
  onHoverIn?: () => void;
  onHoverOut?: () => void;
}) {
  const cls = ['diagram-node-group', hot ? 'is-hot' : ''].filter(Boolean).join(' ');

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onSelect?.(node.id);
    }
  }

  const inner = (
    <g transform={`translate(${node.x},${node.y})`} class={cls} onMouseEnter={onHoverIn} onMouseLeave={onHoverOut}>
      <NodeShape node={node} selected={selected} />
      <LabelLines label={node.label} sublabel={node.sublabel} />
      {onExpand && (
        <g
          class="diagram-expand"
          transform={`translate(${node.width / 2 - 10},${-node.height / 2})`}
          onClick={(e: MouseEvent) => {
            e.stopPropagation();
            e.preventDefault();
            onExpand(node.id);
          }}
          role="button"
          aria-label={`Expand ${node.label}`}
          tabIndex={0}
        >
          <circle r={8} />
          <text text-anchor="middle" dominant-baseline="middle" font-size="12">+</text>
        </g>
      )}
    </g>
  );

  if (node.href) {
    return (
      <a href={node.href} class="diagram-node-anchor">
        {inner}
      </a>
    );
  }

  return (
    <g
      tabIndex={0}
      role="button"
      aria-label={node.label}
      onClick={() => onSelect?.(node.id)}
      onKeyDown={handleKeyDown}
    >
      {inner}
    </g>
  );
}

// ── Arrow markers ─────────────────────────────────────────────────────────────

function Markers({ id }: { id: string }) {
  function marker(kind: string) {
    return (
      <marker
        id={`${id}-${kind}`}
        markerWidth="8"
        markerHeight="6"
        refX="8"
        refY="3"
        orient="auto"
      >
        <path d="M0,0 L8,3 L0,6 Z" class={`diagram-arrow diagram-arrow--${kind}`} />
      </marker>
    );
  }

  return (
    <defs>
      {['next', 'then', 'else', 'loop', 'jump', 'call', 'reference', 'path', 'unknown'].map(
        (kind) => marker(kind)
      )}
    </defs>
  );
}

// ── Diagram ───────────────────────────────────────────────────────────────────

export function Diagram({
  graph,
  rankdir,
  selectedId,
  onSelect,
  onExpand,
  ariaLabel,
}: DiagramProps) {
  const uid = useId();
  const svgRef = useRef<SVGSVGElement>(null);
  const [layout, setLayout] = useState<Layout | null>(null);
  const [copyFeedback, setCopyFeedback] = useState('');
  const [hotId, setHotId] = useState<string | null>(null);

  const { transform, fit, zoomBy } = useViewport(svgRef);

  // Fit only on the first layout for this Diagram instance and when rankdir changes.
  const fittedRef = useRef(false);
  const prevRankdir = useRef(rankdir);

  const fitOnce = useCallback(
    (l: Layout) => {
      const rankdirChanged = rankdir !== prevRankdir.current;
      if (!fittedRef.current || rankdirChanged) {
        fittedRef.current = true;
        prevRankdir.current = rankdir;
        fit(l);
      }
    },
    [rankdir, fit]
  );

  useEffect(() => {
    let cancelled = false;
    setLayout(null);
    layoutGraph(graph, { rankdir }).then((l) => {
      if (!cancelled) {
        setLayout(l);
        fitOnce(l);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [graph, rankdir]);

  // Double-click fits
  function onDblClick() {
    if (layout) fit(layout);
  }

  // Copy as Mermaid
  async function copyMermaid() {
    const text = toMermaid(graph);
    try {
      await navigator.clipboard.writeText(text);
      setCopyFeedback('Copied');
    } catch {
      // Fallback: create a textarea, select and copy
      const ta = document.createElement('textarea');
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
      setCopyFeedback('Copied');
    }
    setTimeout(() => setCopyFeedback(''), 2000);
  }

  // Hover: collect neighbour ids
  function getHotSet(id: string): Set<string> {
    const s = new Set<string>([id]);
    for (const e of graph.edges) {
      if (e.from === id) s.add(e.to);
      if (e.to === id) s.add(e.from);
    }
    return s;
  }

  const hotSet = hotId != null ? getHotSet(hotId) : null;

  if (graph.nodes.length === 0) {
    return <div class="diagram-empty">Nothing to show</div>;
  }

  if (layout == null) {
    return <div class="diagram-loading" aria-busy="true">Loading…</div>;
  }

  const transformStr = `translate(${transform.x},${transform.y}) scale(${transform.k})`;

  return (
    <div class="diagram-container">
      <div class="diagram-toolbar" role="toolbar" aria-label="Diagram controls">
        <button
          type="button"
          onClick={() => fit(layout)}
        >
          Fit
        </button>
        <button
          type="button"
          onClick={() => zoomBy(1.2)}
        >
          Zoom in
        </button>
        <button
          type="button"
          onClick={() => zoomBy(1 / 1.2)}
        >
          Zoom out
        </button>
        <button
          type="button"
          onClick={() => void copyMermaid()}
        >
          {copyFeedback || 'Copy as Mermaid'}
        </button>
      </div>
      <svg
        ref={svgRef}
        class="diagram-svg"
        role="img"
        aria-label={ariaLabel}
        onDblClick={onDblClick}
      >
        <Markers id={uid} />
        <g transform={transformStr}>
          {layout.edges.map((edge, i) => {
            const isHot =
              hotSet != null && (hotSet.has(edge.from) || hotSet.has(edge.to));
            const kindMarkerId = `${uid}-${edge.kind}`;
            return (
              <EdgePath
                key={`e${i}`}
                edge={edge}
                markerId={kindMarkerId}
                hot={isHot}
              />
            );
          })}
          {layout.nodes.map((node) => {
            const isHot = hotSet != null && hotSet.has(node.id);
            return (
              <NodeGroup
                key={node.id}
                node={node}
                selected={node.id === selectedId}
                hot={isHot}
                onSelect={onSelect}
                onExpand={onExpand}
                onHoverIn={() => setHotId(node.id)}
                onHoverOut={() => setHotId(null)}
              />
            );
          })}
        </g>
      </svg>
    </div>
  );
}
