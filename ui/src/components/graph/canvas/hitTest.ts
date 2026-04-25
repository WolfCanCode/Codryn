import type { Point } from './camera';

export type HitNode = {
  id: number;
  x: number;
  y: number;
  r: number;
};

type CellKey = string;

export type HitIndex = {
  cellSize: number;
  cells: Map<CellKey, HitNode[]>;
};

function key(ix: number, iy: number): CellKey {
  return `${ix}\0${iy}`;
}

export function buildHitIndex(nodes: HitNode[], cellSize: number): HitIndex {
  const size = Math.max(4, cellSize);
  const cells = new Map<CellKey, HitNode[]>();
  for (const n of nodes) {
    const ix = Math.floor(n.x / size);
    const iy = Math.floor(n.y / size);
    const k = key(ix, iy);
    const arr = cells.get(k);
    if (arr) arr.push(n);
    else cells.set(k, [n]);
  }
  return { cellSize: size, cells };
}

export function hitTestNearest(index: HitIndex, pWorld: Point, maxDist: number): HitNode | null {
  const { cellSize, cells } = index;
  const ix = Math.floor(pWorld.x / cellSize);
  const iy = Math.floor(pWorld.y / cellSize);
  const r = Math.max(1, Math.ceil(maxDist / cellSize));

  let best: HitNode | null = null;
  let bestD2 = maxDist * maxDist;

  for (let dx = -r; dx <= r; dx++) {
    for (let dy = -r; dy <= r; dy++) {
      const arr = cells.get(key(ix + dx, iy + dy));
      if (!arr) continue;
      for (const n of arr) {
        const ddx = n.x - pWorld.x;
        const ddy = n.y - pWorld.y;
        const d2 = ddx * ddx + ddy * ddy;
        if (d2 <= bestD2) {
          bestD2 = d2;
          best = n;
        }
      }
    }
  }
  return best;
}

