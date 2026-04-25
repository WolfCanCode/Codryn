import type { Camera } from './camera';
import { worldToScreen } from './camera';

export type RenderNode = {
  id: number;
  x: number;
  y: number;
  r: number;
  color: string;
  label?: string;
  name?: string;
};

export type RenderEdge = {
  source: number;
  target: number;
  color?: string;
  alpha?: number;
};

export function renderGraph(opts: {
  ctx: CanvasRenderingContext2D;
  width: number; // CSS pixels
  height: number; // CSS pixels
  dpr?: number;
  camera: Camera;
  nodes: RenderNode[];
  edges: RenderEdge[];
  selectedNodeId?: number | null;
  hoveredNodeId?: number | null;
}) {
  const { ctx, width, height, camera, nodes, edges, selectedNodeId, hoveredNodeId } = opts;
  const dpr = Math.max(1, opts.dpr ?? 1);

  // Clear in device pixels, then draw in CSS pixel coordinates.
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, Math.ceil(width * dpr), Math.ceil(height * dpr));
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

  // Background is handled by parent container; keep canvas transparent.

  // Build quick node lookup for edges.
  const byId = new Map<number, RenderNode>();
  for (const n of nodes) byId.set(n.id, n);

  // Selection neighborhood (1-hop). Used to fade the rest.
  const selected = selectedNodeId ?? null;
  const hovered = hoveredNodeId ?? null;
  const focusId = selected ?? hovered;
  const incidentNodes = new Set<number>();
  const incidentEdges = new Set<number>();
  if (focusId != null) {
    incidentNodes.add(focusId);
    for (let i = 0; i < edges.length; i++) {
      const e = edges[i]!;
      if (e.source === focusId || e.target === focusId) {
        incidentEdges.add(i);
        incidentNodes.add(e.source);
        incidentNodes.add(e.target);
      }
    }
  }

  // Edges
  ctx.lineCap = 'round';
  for (let i = 0; i < edges.length; i++) {
    const e = edges[i]!;
    const s = byId.get(e.source);
    const t = byId.get(e.target);
    if (!s || !t) continue;
    const ps = worldToScreen(camera, s);
    const pt = worldToScreen(camera, t);
    // Black edges (readability), allow per-edge overrides.
    ctx.strokeStyle = e.color ?? 'rgba(0,0,0,0.22)';
    const baseAlpha = e.alpha ?? 1;
    const faded = focusId != null && !incidentEdges.has(i);
    ctx.globalAlpha = faded ? Math.min(0.08, baseAlpha) : baseAlpha;
    // Slightly thicker edges for readability (in CSS pixels).
    ctx.lineWidth = 1.6;
    ctx.beginPath();
    ctx.moveTo(ps.x, ps.y);
    ctx.lineTo(pt.x, pt.y);
    ctx.stroke();
  }
  ctx.globalAlpha = 1;

  // Nodes
  for (const n of nodes) {
    const p = worldToScreen(camera, n);
    const isSelected = selectedNodeId != null && n.id === selectedNodeId;
    const isHovered = hoveredNodeId != null && n.id === hoveredNodeId;
    const isIncident = focusId == null ? true : incidentNodes.has(n.id);
    const rr = Math.max(2, n.r * camera.scale);

    const nodeAlpha = isIncident ? 1 : 0.14;
    ctx.globalAlpha = nodeAlpha;

    ctx.beginPath();
    ctx.fillStyle = n.color || '#71717a';
    ctx.arc(p.x, p.y, rr, 0, Math.PI * 2);
    ctx.fill();

    // White outline (always), stronger when selected/hovered.
    ctx.beginPath();
    ctx.arc(p.x, p.y, rr + 0.25, 0, Math.PI * 2);
    ctx.strokeStyle = 'rgba(0,0,0,0.78)';
    ctx.lineWidth = isSelected || isHovered ? 2.2 : 1.2;
    ctx.stroke();

    // Labels when zoomed in enough, or when selected/hovered.
    const showLabels = focusId != null ? isIncident : camera.scale >= 0.75;
    if (isSelected || isHovered || showLabels) {
      const text = n.name ?? n.label ?? String(n.id);
      const fontSize = isSelected || isHovered ? 14 : 12;
      ctx.font = `${fontSize}px ui-sans-serif, system-ui, -apple-system, Segoe UI`;
      // White labels with dark stroke for readability.
      ctx.lineWidth = 3;
      ctx.strokeStyle = 'rgba(24,24,27,0.55)';
      ctx.textBaseline = 'middle';
      ctx.strokeText(text, p.x + rr + 10, p.y);
      ctx.fillStyle = 'rgba(255,255,255,0.94)';
      ctx.textBaseline = 'middle';
      ctx.fillText(text, p.x + rr + 10, p.y);
    }
  }
  ctx.globalAlpha = 1;
}

