import { useEffect, useMemo, useRef, useState } from 'react';
import { cn } from '@/lib/utils';
import type { GraphData, GraphNode } from '@/lib/types';
import type { Camera } from './camera';
import { fitToBounds, panBy, screenToWorld, worldToScreen, zoomAt } from './camera';
import { buildHitIndex, hitTestNearest } from './hitTest';
import { renderGraph, type RenderEdge, type RenderNode } from './render';
import { runD3ForceLayout } from './layout';

type Props = {
  project: string;
  kind: string;
  data: GraphData | null;
  maxNodes: number;
  className?: string;
  selectedNodeId?: number | null;
  onSelectNode?: (n: GraphNode | null) => void;
};

export function CanvasGraphView({ project, kind, data, maxNodes, className, selectedNodeId, onSelectNode }: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const minimapRef = useRef<HTMLCanvasElement | null>(null);
  const rafRef = useRef<number | null>(null);
  const minimapRafRef = useRef<number | null>(null);

  const [camera, setCamera] = useState<Camera>({ scale: 1, tx: 0, ty: 0 });
  const [hoveredId, setHoveredId] = useState<number | null>(null);
  const [layoutNodes, setLayoutNodes] = useState<Map<number, { x: number; y: number }>>(new Map());
  const [isLayouting, setIsLayouting] = useState(false);
  const abortLayoutRef = useRef<AbortController | null>(null);
  const boundsRef = useRef<{ minX: number; minY: number; maxX: number; maxY: number } | null>(null);
  const [viewSize, setViewSize] = useState<{ width: number; height: number }>({ width: 0, height: 0 });
  const didUserMoveRef = useRef(false);

  const dragRef = useRef<{ active: boolean; lastX: number; lastY: number } | null>(null);
  const hitIndexRef = useRef<ReturnType<typeof buildHitIndex> | null>(null);
  const minimapDragRef = useRef<{ active: boolean } | null>(null);

  const { nodes, edges } = useMemo(() => {
    return { nodes: data?.nodes ?? [], edges: data?.edges ?? [] };
  }, [data]);

  const nodesById = useMemo(() => {
    const m = new Map<number, GraphNode>();
    for (const n of nodes) m.set(n.id, n);
    return m;
  }, [nodes]);

  const renderNodes: RenderNode[] = useMemo(() => {
    return nodes.map((n) => {
      const p = layoutNodes.get(n.id);
      return {
        id: n.id,
        x: p?.x ?? n.x ?? 0,
        y: p?.y ?? n.y ?? 0,
        // Bigger nodes for readability.
        r: Math.max(3, (n.size || 2) * 0.95),
        color: n.color || '#71717a',
        label: n.label,
        name: n.name,
      };
    });
  }, [layoutNodes, nodes]);

  const selectedRenderNode = useMemo(() => {
    if (selectedNodeId == null) return null;
    return renderNodes.find((n) => n.id === selectedNodeId) ?? null;
  }, [renderNodes, selectedNodeId]);

  const renderEdges: RenderEdge[] = useMemo(() => {
    const set = new Set(nodes.map((n) => n.id));
    return edges
      .filter((e) => set.has(e.source) && set.has(e.target))
      .map((e) => ({ source: e.source, target: e.target }));
  }, [edges, nodes]);

  const selectedNeighborhood = useMemo(() => {
    if (selectedNodeId == null) return null;
    const calls: GraphNode[] = [];
    const callers: GraphNode[] = [];
    const seenOut = new Set<number>();
    const seenIn = new Set<number>();
    for (const e of edges) {
      if (e.source === selectedNodeId && !seenOut.has(e.target)) {
        const t = nodesById.get(e.target);
        if (t) {
          calls.push(t);
          seenOut.add(e.target);
        }
      }
      if (e.target === selectedNodeId && !seenIn.has(e.source)) {
        const s = nodesById.get(e.source);
        if (s) {
          callers.push(s);
          seenIn.add(e.source);
        }
      }
    }
    const byName = (a: GraphNode, b: GraphNode) => (a.name ?? '').localeCompare(b.name ?? '');
    calls.sort(byName);
    callers.sort(byName);
    return { calls, callers };
  }, [edges, nodesById, selectedNodeId]);

  function scheduleDraw() {
    if (rafRef.current != null) return;
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      const canvas = canvasRef.current;
      if (!canvas) return;
      const ctx = canvas.getContext('2d');
      if (!ctx) return;
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      const rect = canvas.getBoundingClientRect();
      const cssWidth = Math.max(1, rect.width);
      const cssHeight = Math.max(1, rect.height);
      const width = Math.max(1, Math.round(cssWidth * dpr));
      const height = Math.max(1, Math.round(cssHeight * dpr));
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }

      renderGraph({
        ctx,
        width: cssWidth,
        height: cssHeight,
        dpr,
        camera,
        nodes: renderNodes,
        edges: renderEdges,
        selectedNodeId: selectedNodeId ?? null,
        hoveredNodeId: hoveredId,
      });
    });
  }

  function scheduleMinimapDraw() {
    if (minimapRafRef.current != null) return;
    minimapRafRef.current = requestAnimationFrame(() => {
      minimapRafRef.current = null;
      const canvas = minimapRef.current;
      if (!canvas) return;
      const ctx = canvas.getContext('2d');
      if (!ctx) return;

      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      const rect = canvas.getBoundingClientRect();
      const width = Math.max(1, Math.round(rect.width * dpr));
      const height = Math.max(1, Math.round(rect.height * dpr));
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }

      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, width, height);
      ctx.scale(dpr, dpr);

      // Background
      ctx.fillStyle = 'rgba(244,244,245,0.92)'; // zinc-100-ish
      ctx.fillRect(0, 0, rect.width, rect.height);

      const b = boundsRef.current;
      if (!b) return;
      const pad = 8;
      const bw = Math.max(1e-6, b.maxX - b.minX);
      const bh = Math.max(1e-6, b.maxY - b.minY);
      const sx = (rect.width - pad * 2) / bw;
      const sy = (rect.height - pad * 2) / bh;
      const s = Math.min(sx, sy);

      const ox = pad + (rect.width - pad * 2 - bw * s) / 2;
      const oy = pad + (rect.height - pad * 2 - bh * s) / 2;

      // Nodes (dots)
      ctx.globalAlpha = 0.9;
      for (const n of renderNodes) {
        const x = ox + (n.x - b.minX) * s;
        const y = oy + (n.y - b.minY) * s;
        ctx.beginPath();
        ctx.fillStyle = n.color || '#71717a';
        ctx.arc(x, y, 1.4, 0, Math.PI * 2);
        ctx.fill();
      }
      ctx.globalAlpha = 1;

      // Viewport rectangle (screen -> world -> minimap)
      const main = canvasRef.current;
      if (!main) return;
      const mainRect = main.getBoundingClientRect();
      const w0 = { x: (0 - camera.tx) / camera.scale, y: (0 - camera.ty) / camera.scale };
      const w1 = { x: (mainRect.width - camera.tx) / camera.scale, y: (mainRect.height - camera.ty) / camera.scale };
      const vx0 = ox + (w0.x - b.minX) * s;
      const vy0 = oy + (w0.y - b.minY) * s;
      const vx1 = ox + (w1.x - b.minX) * s;
      const vy1 = oy + (w1.y - b.minY) * s;

      const x0 = Math.min(vx0, vx1);
      const y0 = Math.min(vy0, vy1);
      const x1 = Math.max(vx0, vx1);
      const y1 = Math.max(vy0, vy1);

      ctx.strokeStyle = 'rgba(24,24,27,0.7)';
      ctx.lineWidth = 1;
      ctx.setLineDash([5, 4]);
      ctx.strokeRect(x0, y0, Math.max(8, x1 - x0), Math.max(8, y1 - y0));
      ctx.setLineDash([]);
    });
  }

  // Rebuild hit index when positions change.
  useEffect(() => {
    const hitNodes = renderNodes.map((n) => ({ id: n.id, x: n.x, y: n.y, r: n.r }));
    hitIndexRef.current = buildHitIndex(hitNodes, 32);
  }, [renderNodes]);

  // Run layout when data changes.
  useEffect(() => {
    let cancelled = false;
    if (!data || !data.nodes?.length) {
      const t = window.setTimeout(() => setLayoutNodes(new Map()), 0);
      return () => window.clearTimeout(t);
      return;
    }

    void (async () => {
      abortLayoutRef.current?.abort();
      const controller = new AbortController();
      abortLayoutRef.current = controller;
      setIsLayouting(true);

      const res = await runD3ForceLayout({
        project,
        kind,
        data,
        maxNodes,
        signal: controller.signal,
      });
      if (cancelled) return;
      if (controller.signal.aborted) return;
      const m = new Map<number, { x: number; y: number }>();
      for (const n of res.nodes) m.set(n.id, { x: n.x, y: n.y });
      setLayoutNodes(m);
      setIsLayouting(false);

      // Fit camera on first layout (when camera is still default-ish).
      const xs = res.nodes.map((n) => n.x);
      const ys = res.nodes.map((n) => n.y);
      const minX = Math.min(...xs);
      const maxX = Math.max(...xs);
      const minY = Math.min(...ys);
      const maxY = Math.max(...ys);
      boundsRef.current = { minX, minY, maxX, maxY };
      const canvas = canvasRef.current;
      if (canvas) {
        const rect = canvas.getBoundingClientRect();
        setCamera(fitToBounds({ width: rect.width, height: rect.height, minX, minY, maxX, maxY, padding: 28 }));
      }
    })();

    return () => {
      cancelled = true;
      abortLayoutRef.current?.abort();
      setIsLayouting(false);
    };
  }, [data, kind, maxNodes, project]);

  useEffect(() => {
    scheduleDraw();
    scheduleMinimapDraw();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [camera, renderEdges, renderNodes, hoveredId, selectedNodeId]);

  // Use a native wheel listener with passive:false so preventDefault works (no page scroll + no console warning).
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      didUserMoveRef.current = true;
      const rect = canvas.getBoundingClientRect();
      const pScreen = { x: e.clientX - rect.left, y: e.clientY - rect.top };
      const delta = -e.deltaY;
      // Faster zoom response (trackpads feel more immediate).
      const k = Math.exp(delta * 0.0024);
      setCamera((c) => zoomAt(c, pScreen, c.scale * k));
    };

    canvas.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      canvas.removeEventListener('wheel', onWheel as EventListener);
    };
  }, []);

  // Track canvas size so we can auto-fit once layout is ready.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const update = () => {
      const rect = canvas.getBoundingClientRect();
      setViewSize({ width: Math.round(rect.width), height: Math.round(rect.height) });
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(canvas);
    return () => ro.disconnect();
  }, []);

  // Auto-fit when we have bounds + a real canvas size (and user hasn't interacted yet).
  useEffect(() => {
    const b = boundsRef.current;
    if (!b) return;
    if (!viewSize.width || !viewSize.height) return;
    if (didUserMoveRef.current) return;
    setCamera(fitToBounds({ width: viewSize.width, height: viewSize.height, ...b, padding: 28 }));
  }, [viewSize]);

  return (
    <div className={cn('relative h-full w-full', className)}>
      <canvas
        ref={canvasRef}
        className="h-full w-full"
        onPointerDown={(e) => {
          (e.currentTarget as HTMLCanvasElement).setPointerCapture(e.pointerId);
          dragRef.current = { active: true, lastX: e.clientX, lastY: e.clientY };
        }}
        onPointerMove={(e) => {
          const canvas = e.currentTarget as HTMLCanvasElement;
          const rect = canvas.getBoundingClientRect();
          const pScreen = { x: e.clientX - rect.left, y: e.clientY - rect.top };

          // Drag to pan
          const d = dragRef.current;
          if (d?.active) {
            didUserMoveRef.current = true;
            const dx = e.clientX - d.lastX;
            const dy = e.clientY - d.lastY;
            d.lastX = e.clientX;
            d.lastY = e.clientY;
            setCamera((c) => panBy(c, dx, dy));
            return;
          }

          // Hover hit test
          const idx = hitIndexRef.current;
          if (!idx) return;
          const world = screenToWorld(camera, pScreen);
          const hit = hitTestNearest(idx, world, 12 / camera.scale);
          const next = hit?.id ?? null;
          setHoveredId((prev) => (prev === next ? prev : next));
        }}
        onPointerUp={(e) => {
          dragRef.current = null;
          // Click selection (if not dragging)
          const canvas = e.currentTarget as HTMLCanvasElement;
          const rect = canvas.getBoundingClientRect();
          const pScreen = { x: e.clientX - rect.left, y: e.clientY - rect.top };
          const idx = hitIndexRef.current;
          if (!idx) return;
          const world = screenToWorld(camera, pScreen);
          const hit = hitTestNearest(idx, world, 12 / camera.scale);
          const node = hit ? nodesById.get(hit.id) ?? null : null;
          onSelectNode?.(node);
        }}
      />

      {/* Selected node popup */}
      {selectedRenderNode ? (
        (() => {
          const p = worldToScreen(camera, selectedRenderNode);
          const node = nodesById.get(selectedRenderNode.id);
          if (!node) return null;
          // Clamp within this view, without relying on window size.
          const left = Math.max(12, Math.min(p.x + 14, 980));
          const top = Math.max(12, Math.min(p.y + 14, 560));
          return (
            <div
              className="absolute z-20 w-[320px] rounded-xl border border-zinc-200 bg-white/95 p-3 shadow-xl backdrop-blur-[2px]"
              style={{ left, top }}
            >
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="truncate text-sm font-semibold text-zinc-900">{node.name}</div>
                  <div className="mt-0.5 truncate text-[11px] text-zinc-500">{node.label}</div>
                </div>
                <button
                  type="button"
                  className="rounded-md px-2 py-1 text-xs text-zinc-600 hover:bg-zinc-100"
                  onClick={() => onSelectNode?.(null)}
                >
                  Close
                </button>
              </div>
              {node.file_path ? (
                <div className="mt-2 rounded-lg bg-zinc-50 px-2 py-1.5 text-[11px] font-mono text-zinc-700">
                  {node.file_path}
                </div>
              ) : null}

              {selectedNeighborhood ? (
                <div className="mt-2 grid grid-cols-2 gap-2">
                  <div className="rounded-lg border border-zinc-200 bg-white px-2 py-1.5">
                    <div className="text-[11px] font-semibold text-zinc-700">Calls</div>
                    <div className="mt-1 space-y-0.5">
                      {selectedNeighborhood.calls.slice(0, 8).map((n) => (
                        <div key={n.id} className="truncate text-[11px] text-zinc-600">
                          {n.name ?? n.label ?? n.id}
                        </div>
                      ))}
                      {selectedNeighborhood.calls.length > 8 ? (
                        <div className="text-[11px] text-zinc-500">+{selectedNeighborhood.calls.length - 8} more</div>
                      ) : null}
                      {selectedNeighborhood.calls.length === 0 ? (
                        <div className="text-[11px] text-zinc-400">None</div>
                      ) : null}
                    </div>
                  </div>
                  <div className="rounded-lg border border-zinc-200 bg-white px-2 py-1.5">
                    <div className="text-[11px] font-semibold text-zinc-700">Called by</div>
                    <div className="mt-1 space-y-0.5">
                      {selectedNeighborhood.callers.slice(0, 8).map((n) => (
                        <div key={n.id} className="truncate text-[11px] text-zinc-600">
                          {n.name ?? n.label ?? n.id}
                        </div>
                      ))}
                      {selectedNeighborhood.callers.length > 8 ? (
                        <div className="text-[11px] text-zinc-500">
                          +{selectedNeighborhood.callers.length - 8} more
                        </div>
                      ) : null}
                      {selectedNeighborhood.callers.length === 0 ? (
                        <div className="text-[11px] text-zinc-400">None</div>
                      ) : null}
                    </div>
                  </div>
                </div>
              ) : null}
            </div>
          );
        })()
      ) : null}

      {/* Minimap */}
      <div className="pointer-events-none absolute right-6 top-6 z-10">
        <div className="rounded-xl border border-zinc-200 bg-white/80 p-2 shadow-lg backdrop-blur-[2px]">
          <canvas
            ref={minimapRef}
            className="pointer-events-auto h-[140px] w-[220px] rounded-lg bg-transparent"
            onPointerDown={(e) => {
              (e.currentTarget as HTMLCanvasElement).setPointerCapture(e.pointerId);
              minimapDragRef.current = { active: true };
            }}
            onPointerMove={(e) => {
              const d = minimapDragRef.current;
              if (!d?.active) return;
              const b = boundsRef.current;
              const mini = minimapRef.current;
              const main = canvasRef.current;
              if (!b || !mini || !main) return;

              const rect = mini.getBoundingClientRect();
              const pad = 8;
              const bw = Math.max(1e-6, b.maxX - b.minX);
              const bh = Math.max(1e-6, b.maxY - b.minY);
              const sx = (rect.width - pad * 2) / bw;
              const sy = (rect.height - pad * 2) / bh;
              const s = Math.min(sx, sy);
              const ox = pad + (rect.width - pad * 2 - bw * s) / 2;
              const oy = pad + (rect.height - pad * 2 - bh * s) / 2;

              const x = e.clientX - rect.left;
              const y = e.clientY - rect.top;
              const wx = b.minX + (x - ox) / s;
              const wy = b.minY + (y - oy) / s;

              const mainRect = main.getBoundingClientRect();
              // center camera on this world point
              setCamera((c) => ({
                ...c,
                tx: mainRect.width / 2 - wx * c.scale,
                ty: mainRect.height / 2 - wy * c.scale,
              }));
            }}
            onPointerUp={() => {
              minimapDragRef.current = null;
            }}
          />
        </div>
      </div>

      {!data ? (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-zinc-500">
          No graph data
        </div>
      ) : null}

      {/* Skeleton-ish loading overlay */}
      {data && isLayouting ? (
        <div className="pointer-events-none absolute inset-0 z-30">
          <div className="absolute inset-0 bg-linear-to-b from-white/70 via-white/35 to-white/60 backdrop-blur-[1px]" />
          <div className="absolute inset-0 opacity-70 motion-reduce:opacity-60">
            <div className="absolute inset-0 animate-pulse motion-reduce:animate-none">
              <div className="absolute left-[12%] top-[18%] h-2 w-44 rounded-full bg-zinc-200/80" />
              <div className="absolute left-[14%] top-[26%] h-2 w-64 rounded-full bg-zinc-200/70" />
              <div className="absolute left-[10%] top-[40%] h-2 w-52 rounded-full bg-zinc-200/70" />
              <div className="absolute left-[18%] top-[52%] h-2 w-40 rounded-full bg-zinc-200/60" />
            </div>
            <div className="absolute inset-0 mask-[linear-gradient(to_right,transparent,black,transparent)]">
              <div className="absolute -left-[40%] top-0 h-full w-[60%] animate-[shimmer_1.2s_linear_infinite] bg-linear-to-r from-transparent via-white/55 to-transparent motion-reduce:hidden" />
            </div>
          </div>
          <div className="absolute inset-0 flex items-center justify-center">
            <div className="rounded-2xl border border-zinc-200 bg-white/85 px-4 py-2 text-xs font-medium text-zinc-700 shadow-sm">
              Laying out graph…
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}

