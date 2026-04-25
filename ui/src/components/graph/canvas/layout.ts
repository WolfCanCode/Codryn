import type { Simulation } from 'd3-force';
import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation } from 'd3-force';
import type { GraphData, GraphEdge, GraphNode } from '@/lib/types';

type LayoutNode = { id: number; x: number; y: number; vx?: number; vy?: number; r: number };
type LayoutLink = { source: number; target: number };

function cacheKey(project: string, kind: string, order: number) {
  return `codryn:canvasLayout:${kind}:${project}:${order}`;
}

export function readLayoutCache(project: string, kind: string, order: number): Map<number, { x: number; y: number }> {
  try {
    const raw = localStorage.getItem(cacheKey(project, kind, order));
    if (!raw) return new Map();
    const parsed = JSON.parse(raw) as { v: number; nodes: Record<string, { x: number; y: number }> };
    if (!parsed || parsed.v !== 1) return new Map();
    const m = new Map<number, { x: number; y: number }>();
    for (const [k, v] of Object.entries(parsed.nodes || {})) {
      const id = Number(k);
      if (!Number.isFinite(id)) continue;
      if (!v || !Number.isFinite(v.x) || !Number.isFinite(v.y)) continue;
      m.set(id, { x: v.x, y: v.y });
    }
    return m;
  } catch {
    return new Map();
  }
}

export function writeLayoutCache(project: string, kind: string, order: number, nodes: LayoutNode[]) {
  try {
    const out: Record<string, { x: number; y: number }> = {};
    for (const n of nodes) out[String(n.id)] = { x: n.x, y: n.y };
    localStorage.setItem(cacheKey(project, kind, order), JSON.stringify({ v: 1, nodes: out }));
  } catch {
    // ignore
  }
}

export function buildLayoutInput(data: GraphData, maxNodes: number): { nodes: GraphNode[]; edges: GraphEdge[] } {
  const nodes = [...(data.nodes ?? [])];
  const edges = [...(data.edges ?? [])];
  if (nodes.length <= maxNodes) return { nodes, edges };

  // Deterministic cap: take first N by size desc then id asc.
  const sorted = nodes
    .slice()
    .sort((a, b) => (b.size !== a.size ? b.size - a.size : a.id - b.id))
    .slice(0, maxNodes);
  const allowed = new Set(sorted.map((n) => n.id));
  const filteredEdges = edges.filter((e) => allowed.has(e.source) && allowed.has(e.target));
  return { nodes: sorted, edges: filteredEdges };
}

export async function runD3ForceLayout(opts: {
  project: string;
  kind: string;
  data: GraphData;
  maxNodes: number;
  timeBudgetMs?: number;
  onTick?: (nodes: LayoutNode[]) => void;
  signal?: AbortSignal;
}): Promise<{ nodes: LayoutNode[]; edges: LayoutLink[] }> {
  const { project, kind, data, maxNodes } = opts;
  const { nodes, edges } = buildLayoutInput(data, maxNodes);
  const order = nodes.length;

  const cached = readLayoutCache(project, kind, order);
  const layoutNodes: LayoutNode[] = nodes.map((n) => {
    const c = cached.get(n.id);
    const x = c?.x ?? (Number.isFinite(n.x) ? n.x : (Math.random() - 0.5) * 10);
    const y = c?.y ?? (Number.isFinite(n.y) ? n.y : (Math.random() - 0.5) * 10);
    // Larger collide radius → more spacing / readability.
    const r = Math.max(2.4, (n.size || 2) * 1.25);
    return { id: n.id, x, y, r };
  });

  // Sample links for performance: cap per node.
  const maxLinksPerNode = order > 10_000 ? 1 : 2;
  const perNode = new Map<number, number>();
  const sampled: LayoutLink[] = [];
  for (const e of edges) {
    const s = e.source;
    const t = e.target;
    const sc = perNode.get(s) ?? 0;
    const tc = perNode.get(t) ?? 0;
    if (sc >= maxLinksPerNode || tc >= maxLinksPerNode) continue;
    perNode.set(s, sc + 1);
    perNode.set(t, tc + 1);
    sampled.push({ source: s, target: t });
    if (sampled.length >= order * maxLinksPerNode) break;
  }

  // Stronger repulsion → more spacing.
  const chargeStrength = order > 10_000 ? -12 : order > 6_000 ? -16 : order > 2_500 ? -22 : -30;
  const timeBudgetMs =
    opts.timeBudgetMs ?? (order > 10_000 ? 900 : order > 6_000 ? 1100 : order > 2_500 ? 1400 : 1800);

  let sim: Simulation<LayoutNode, undefined> | null = null;
  try {
    sim = forceSimulation(layoutNodes)
      .alpha(1)
      .alphaMin(0.06)
      .alphaDecay(order > 6_000 ? 0.08 : 0.06)
      .velocityDecay(order > 6_000 ? 0.55 : 0.45)
      .force('charge', forceManyBody<LayoutNode>().strength(chargeStrength))
      .force('center', forceCenter<LayoutNode>(0, 0))
      .force(
        'link',
        forceLink<LayoutNode, LayoutLink>(sampled)
          .id((d) => d.id)
          .distance(order > 10_000 ? 12 : order > 6_000 ? 16 : 22)
          .strength(order > 10_000 ? 0.03 : order > 6_000 ? 0.05 : 0.07),
      )
      .force('collide', forceCollide<LayoutNode>().radius((d) => d.r).iterations(order > 6000 ? 1 : 2));

    const stopAt = performance.now() + timeBudgetMs;
    let lastNotify = 0;

    const nextFrame = () =>
      new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

    while (performance.now() < stopAt) {
      if (opts.signal?.aborted) break;

      // Tick in small chunks to avoid blocking the main thread.
      const chunkStop = performance.now() + 10; // ~10ms budget per frame
      while (performance.now() < chunkStop && performance.now() < stopAt) {
        sim.tick();
      }

      const now = performance.now();
      if (opts.onTick && now - lastNotify > 80) {
        lastNotify = now;
        opts.onTick(layoutNodes);
      }

      await nextFrame();
    }
  } finally {
    sim?.stop();
    writeLayoutCache(project, kind, order, layoutNodes);
  }

  return { nodes: layoutNodes, edges: sampled };
}

