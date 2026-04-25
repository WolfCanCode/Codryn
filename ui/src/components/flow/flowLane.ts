import type { FlowNode, SankeyLane } from './flowTypes';

export type FlowMode = 'route' | 'ui';

export const ROUTE_LANES = ['route', 'controller', 'service', 'repository', 'dto', 'unknown'] as const satisfies readonly SankeyLane[];
export const UI_LANES = ['component', 'service', 'framework', 'unknown'] as const satisfies readonly SankeyLane[];

export function lanesForMode(mode: FlowMode): readonly SankeyLane[] {
  return mode === 'route' ? ROUTE_LANES : UI_LANES;
}

export function laneForNode(mode: FlowMode, node: FlowNode | null | undefined): SankeyLane {
  const layer = (node?.layer ?? '').trim().toLowerCase();
  if (!layer) return 'unknown';

  if (mode === 'route') {
    if (layer === 'route') return 'route';
    if (layer === 'controller') return 'controller';
    if (layer === 'service') return 'service';
    if (layer === 'repository') return 'repository';
    if (layer === 'dto') return 'dto';
    return 'unknown';
  }

  if (layer === 'component') return 'component';
  if (layer === 'service') return 'service';
  if (layer === 'react' || layer === 'vue' || layer === 'solid') return 'framework';
  return 'unknown';
}
