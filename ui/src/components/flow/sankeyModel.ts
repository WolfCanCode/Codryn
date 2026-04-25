import type { BackendFlowResponse, FlowEdge, FlowNode, FrontendFlowResponse, SankeyGraph, SankeyLane, SankeyLink, SankeyNode } from './flowTypes';
import { laneForNode, lanesForMode, type FlowMode } from './flowLane';

function normalizeId(value: string | number | { id?: string | number } | null | undefined): string | null {
  if (value == null) return null;
  if (typeof value === 'object') {
    if ('id' in value && value.id != null) return String(value.id);
    return null;
  }
  return String(value);
}

function sankeyNodeId(lane: SankeyLane, flowId: string): string {
  return `${lane}\0${flowId}`;
}

function edgeTypeOf(edge: FlowEdge): string {
  const t = (edge.type ?? '').trim();
  return t || 'CALLS';
}

export function toSankeyGraph(
  mode: FlowMode,
  flow: BackendFlowResponse | FrontendFlowResponse,
): SankeyGraph {
  const lanes = lanesForMode(mode);

  const nodesByFlowId = new Map<string, FlowNode>();
  for (const n of flow.nodes ?? []) {
    const id = normalizeId(n.id);
    if (id) nodesByFlowId.set(id, n);
  }

  const sankeyNodes = new Map<string, SankeyNode>();
  const usedNodeIds = new Set<string>();

  const linkAgg = new Map<
    string,
    { source: string; target: string; weight: number; edgeTypes: Set<string> }
  >();

  const edges = flow.edges ?? [];
  for (const e of edges) {
    const sourceFlowId = normalizeId(e.source);
    const targetFlowId = normalizeId(e.target);
    if (!sourceFlowId || !targetFlowId) continue;

    const sourceNode = nodesByFlowId.get(sourceFlowId);
    const targetNode = nodesByFlowId.get(targetFlowId);

    const sourceLane = laneForNode(mode, sourceNode);
    const targetLane = laneForNode(mode, targetNode);

    const sourceSid = sankeyNodeId(sourceLane, sourceFlowId);
    const targetSid = sankeyNodeId(targetLane, targetFlowId);

    if (!sankeyNodes.has(sourceSid)) {
      sankeyNodes.set(sourceSid, {
        id: sourceSid,
        title: sourceNode?.name ?? sourceFlowId,
        lane: sourceLane,
        file_path: sourceNode?.file_path,
        rawNodeId: sourceFlowId,
      });
    }
    if (!sankeyNodes.has(targetSid)) {
      sankeyNodes.set(targetSid, {
        id: targetSid,
        title: targetNode?.name ?? targetFlowId,
        lane: targetLane,
        file_path: targetNode?.file_path,
        rawNodeId: targetFlowId,
      });
    }

    usedNodeIds.add(sourceSid);
    usedNodeIds.add(targetSid);

    const linkKey = `${sourceSid}\0${targetSid}`;
    const edgeType = edgeTypeOf(e);

    const prev = linkAgg.get(linkKey);
    if (prev) {
      prev.weight += 1;
      prev.edgeTypes.add(edgeType);
    } else {
      linkAgg.set(linkKey, {
        source: sourceSid,
        target: targetSid,
        weight: 1,
        edgeTypes: new Set([edgeType]),
      });
    }
  }

  const links: SankeyLink[] = [...linkAgg.values()]
    .filter((l) => l.weight > 0)
    .map((l) => ({
      source: l.source,
      target: l.target,
      weight: l.weight,
      edgeTypes: [...l.edgeTypes].sort(),
    }))
    .sort((a, b) => (a.source === b.source ? a.target.localeCompare(b.target) : a.source.localeCompare(b.source)));

  const laneIndex = new Map<SankeyLane, number>(lanes.map((l, i) => [l, i]));
  const nodes: SankeyNode[] = [...sankeyNodes.values()]
    .filter((n) => usedNodeIds.has(n.id))
    .sort((a, b) => {
      const la = laneIndex.get(a.lane) ?? 999;
      const lb = laneIndex.get(b.lane) ?? 999;
      if (la !== lb) return la - lb;
      return a.title.localeCompare(b.title);
    });

  return { nodes, links, lanes: [...lanes] };
}
