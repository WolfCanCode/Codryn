import { useMemo, useState } from 'react';
import { cn } from '@/lib/utils';
import type { SankeyGraph, SankeyLane, SankeyLink, SankeyNode } from './flowTypes';

type Props = {
  graph: SankeyGraph | null;
  className?: string;
  selectedNodeId?: string;
  onSelectNode?: (id: string) => void;
};

type LayoutNode = SankeyNode & {
  x: number;
  y: number;
  w: number;
  h: number;
};

function clamp(n: number, min: number, max: number) {
  return Math.min(max, Math.max(min, n));
}

function laneLabel(lane: SankeyLane): string {
  switch (lane) {
    case 'route':
      return 'Route';
    case 'controller':
      return 'Controller';
    case 'service':
      return 'Service';
    case 'repository':
      return 'Repository';
    case 'dto':
      return 'DTO';
    case 'component':
      return 'Component';
    case 'framework':
      return 'Framework';
    case 'unknown':
      return 'Unknown';
    default:
      return String(lane);
  }
}

function linkKey(l: SankeyLink) {
  return `${l.source}\0${l.target}\0${l.weight}\0${l.edgeTypes.join(',')}`;
}

export function FlowSankeyView({ graph, className, selectedNodeId, onSelectNode }: Props) {
  const [hoveredLink, setHoveredLink] = useState<{ key: string; source: string; target: string } | null>(null);

  const { lanes, nodesById, laneNodes, size } = useMemo(() => {
    const empty = {
      lanes: [] as SankeyLane[],
      nodesById: new Map<string, LayoutNode>(),
      laneNodes: new Map<SankeyLane, LayoutNode[]>(),
      size: { width: 600, height: 360 },
    };

    if (!graph) return empty;

    const lanes = [...graph.lanes];

    // Layout constants (tuned for readability and determinism).
    const marginX = 20;
    const marginY = 16;
    const laneGap = 22;
    const laneWidth = 220;
    const laneHeaderH = 26;

    const nodeW = 190;
    const nodeH = 30;
    const nodeGap = 10;

    const incidentWeight = new Map<string, number>();
    for (const l of graph.links) {
      incidentWeight.set(l.source, (incidentWeight.get(l.source) ?? 0) + l.weight);
      incidentWeight.set(l.target, (incidentWeight.get(l.target) ?? 0) + l.weight);
    }

    const laneIndex = new Map<SankeyLane, number>(lanes.map((l, i) => [l, i]));
    const laneNodes = new Map<SankeyLane, SankeyNode[]>();
    for (const n of graph.nodes) {
      const lane = n.lane;
      if (!laneNodes.has(lane)) laneNodes.set(lane, []);
      laneNodes.get(lane)!.push(n);
    }

    // Deterministic ordering: lanes are fixed by graph.lanes; nodes are sorted by incident weight desc,
    // then title asc, then id asc.
    const orderedLaneNodes = new Map<SankeyLane, SankeyNode[]>();
    for (const lane of lanes) {
      const ns = (laneNodes.get(lane) ?? []).slice();
      ns.sort((a, b) => {
        const wa = incidentWeight.get(a.id) ?? 0;
        const wb = incidentWeight.get(b.id) ?? 0;
        if (wa !== wb) return wb - wa;
        const t = a.title.localeCompare(b.title);
        if (t !== 0) return t;
        return a.id.localeCompare(b.id);
      });
      orderedLaneNodes.set(lane, ns);
    }

    const nodesById = new Map<string, LayoutNode>();

    let maxLaneContentH = 0;
    for (const lane of lanes) {
      const count = orderedLaneNodes.get(lane)?.length ?? 0;
      const h = count === 0 ? 0 : count * nodeH + (count - 1) * nodeGap;
      maxLaneContentH = Math.max(maxLaneContentH, h);
    }

    const height = Math.max(220, marginY * 2 + laneHeaderH + (maxLaneContentH ? maxLaneContentH + 10 : 0));
    const width = Math.max(520, marginX * 2 + lanes.length * laneWidth + Math.max(0, lanes.length - 1) * laneGap);

    for (const lane of lanes) {
      const idx = laneIndex.get(lane) ?? 0;
      const laneX = marginX + idx * (laneWidth + laneGap);
      const nodeX = laneX + Math.floor((laneWidth - nodeW) / 2);
      let y = marginY + laneHeaderH;

      for (const n of orderedLaneNodes.get(lane) ?? []) {
        const ln: LayoutNode = {
          ...n,
          x: nodeX,
          y,
          w: nodeW,
          h: nodeH,
        };
        nodesById.set(n.id, ln);
        y += nodeH + nodeGap;
      }
    }

    const laneLayoutNodes = new Map<SankeyLane, LayoutNode[]>();
    for (const lane of lanes) {
      laneLayoutNodes.set(
        lane,
        (orderedLaneNodes.get(lane) ?? []).map((n) => nodesById.get(n.id)!).filter(Boolean),
      );
    }

    return { lanes, nodesById, laneNodes: laneLayoutNodes, size: { width, height } };
  }, [graph]);

  if (!graph) {
    return (
      <div
        className={cn(
          'flex h-full w-full items-center justify-center rounded-lg border bg-background text-sm text-muted-foreground',
          className,
        )}
      >
        Select an anchor to render a Sankey view.
      </div>
    );
  }

  const hasRenderable = graph.nodes.length > 0 && graph.links.length > 0;
  if (!hasRenderable) {
    return (
      <div
        className={cn(
          'flex h-full w-full items-center justify-center rounded-lg border bg-background text-sm text-muted-foreground',
          className,
        )}
      >
        No flow data
      </div>
    );
  }

  const selected = selectedNodeId?.trim() ? selectedNodeId : undefined;

  function isIncident(l: SankeyLink) {
    if (selected) return l.source === selected || l.target === selected;
    if (!hoveredLink) return true;
    return (
      l.source === hoveredLink.source ||
      l.source === hoveredLink.target ||
      l.target === hoveredLink.source ||
      l.target === hoveredLink.target
    );
  }

  function laneX(idx: number) {
    const marginX = 20;
    const laneGap = 22;
    const laneWidth = 220;
    return marginX + idx * (laneWidth + laneGap);
  }

  return (
    <div className={cn('h-full w-full overflow-auto rounded-lg border bg-background', className)}>
      <svg
        viewBox={`0 0 ${size.width} ${size.height}`}
        className="block h-full w-full"
        role="img"
        aria-label="Flow Sankey graph"
      >
        {/* Lane headers */}
        {lanes.map((lane, idx) => (
          <g key={lane} transform={`translate(${laneX(idx)}, 12)`}>
            <text
              x={0}
              y={0}
              dominantBaseline="hanging"
              className="select-none fill-muted-foreground text-[11px] font-medium"
            >
              {laneLabel(lane)}
            </text>
          </g>
        ))}

        {/* Links */}
        <g>
          {graph.links.map((l) => {
            const s = nodesById.get(l.source);
            const t = nodesById.get(l.target);
            if (!s || !t) return null;

            const sx = s.x + s.w;
            const sy = s.y + s.h / 2;
            const tx = t.x;
            const ty = t.y + t.h / 2;

            const dx = tx - sx;
            const c1x = sx + dx * 0.5;
            const c2x = tx - dx * 0.5;

            const d = `M ${sx} ${sy} C ${c1x} ${sy}, ${c2x} ${ty}, ${tx} ${ty}`;
            const key = linkKey(l);
            const strokeW = clamp(l.weight, 1, 10);

            const incident = isIncident(l);
            const isHovered = hoveredLink?.key === key;
            const base = incident ? 0.45 : selected || hoveredLink ? 0.08 : 0.12;
            const opacity = isHovered ? Math.min(0.9, base + 0.25) : base;

            return (
              <path
                key={key}
                d={d}
                fill="none"
                stroke="currentColor"
                className={cn(
                  'cursor-pointer text-slate-500 transition-[opacity,color] duration-150',
                  isHovered && 'text-slate-700',
                )}
                strokeWidth={strokeW}
                strokeLinecap="round"
                opacity={opacity}
                onMouseEnter={() => setHoveredLink({ key, source: l.source, target: l.target })}
                onMouseLeave={() => setHoveredLink(null)}
              />
            );
          })}
        </g>

        {/* Nodes */}
        <g>
          {lanes.map((lane) =>
            (laneNodes.get(lane) ?? []).map((n) => {
              const isSelected = selected === n.id;
              const dim = selected && !isSelected ? 0.55 : 1;

              return (
                <g
                  key={n.id}
                  transform={`translate(${n.x}, ${n.y})`}
                  className="cursor-pointer"
                  opacity={dim}
                  onClick={() => onSelectNode?.(n.id)}
                  role="button"
                  aria-label={`Select ${n.title}`}
                >
                  <rect
                    x={0}
                    y={0}
                    width={n.w}
                    height={n.h}
                    rx={8}
                    ry={8}
                    className={cn(
                      'fill-white stroke-slate-200',
                      'dark:fill-slate-950 dark:stroke-slate-800',
                      'transition-[fill,stroke] duration-150',
                      isSelected && 'stroke-slate-900 dark:stroke-slate-100',
                    )}
                    strokeWidth={isSelected ? 2 : 1}
                  />
                  <text
                    x={10}
                    y={n.h / 2}
                    dominantBaseline="middle"
                    className={cn(
                      'select-none fill-slate-900 text-[12px] font-medium',
                      'dark:fill-slate-100',
                    )}
                  >
                    {n.title}
                  </text>
                </g>
              );
            }),
          )}
        </g>
      </svg>
    </div>
  );
}

