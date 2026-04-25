export type Camera = {
  // screen = world * scale + {tx,ty}
  scale: number;
  tx: number;
  ty: number;
};

export type Point = { x: number; y: number };

export function worldToScreen(cam: Camera, p: Point): Point {
  return { x: p.x * cam.scale + cam.tx, y: p.y * cam.scale + cam.ty };
}

export function screenToWorld(cam: Camera, p: Point): Point {
  return { x: (p.x - cam.tx) / cam.scale, y: (p.y - cam.ty) / cam.scale };
}

export function zoomAt(cam: Camera, anchorScreen: Point, nextScale: number): Camera {
  const world = screenToWorld(cam, anchorScreen);
  const scale = clamp(nextScale, 0.05, 20);
  // keep world point under cursor stable
  const tx = anchorScreen.x - world.x * scale;
  const ty = anchorScreen.y - world.y * scale;
  return { scale, tx, ty };
}

export function panBy(cam: Camera, dx: number, dy: number): Camera {
  return { ...cam, tx: cam.tx + dx, ty: cam.ty + dy };
}

export function clamp(n: number, a: number, b: number) {
  return Math.max(a, Math.min(b, n));
}

export function fitToBounds(opts: {
  width: number;
  height: number;
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
  padding?: number;
}): Camera {
  const { width, height, minX, minY, maxX, maxY } = opts;
  const padding = opts.padding ?? 24;
  const w = Math.max(1e-6, maxX - minX);
  const h = Math.max(1e-6, maxY - minY);
  const sx = (width - padding * 2) / w;
  const sy = (height - padding * 2) / h;
  // Slightly zoom in vs strict fit for readability.
  const scale = clamp(Math.min(sx, sy) * 1.12, 0.05, 20);
  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  return { scale, tx: width / 2 - cx * scale, ty: height / 2 - cy * scale };
}

