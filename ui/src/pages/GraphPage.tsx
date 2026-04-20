import { useCallback, useEffect, useRef, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import type { Attributes } from 'graphology-types';
import Sigma from 'sigma';
import type { NodeLabelDrawingFunction } from 'sigma/rendering';
import { createNodeBorderProgram } from '@sigma/node-border';
import { MultiDirectedGraph } from 'graphology';
import forceAtlas2 from 'graphology-layout-forceatlas2';
import FA2Layout from 'graphology-layout-forceatlas2/worker';
import { Plus, Minus, Maximize, MapPin, X, Loader2, AlertCircle, Search, GitBranch, Box, ArrowRight } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Badge } from '@/components/ui/badge';
import { fetchLayout, fetchRoutes, fetchBackendFlow, fetchComponents, fetchFrontendFlow } from '@/lib/rpc';
import type { GraphData, GraphNode } from '@/lib/types';

const MINIMAP_WIDTH = 176;
const MINIMAP_HEIGHT = 132;
const DETAIL_PANEL_WIDTH = 268;
const DETAIL_PANEL_HEIGHT = 300;

const NODE_COLORS: Record<string, string> = {
  Project: '#c62828',
  Folder: '#37474f',
  File: '#546e7a',
  Module: '#7b1fa2',
  Class: '#e64a19',
  Function: '#1976d2',
  Method: '#388e3c',
  Interface: '#f9a825',
  Route: '#00695c',
  Selector: '#ab47bc',
};
const EDGE_COLORS: Record<string, string> = {
  CALLS: '#1976d2',
  IMPORTS: '#388e3c',
  INHERITS: '#e64a19',
  IMPLEMENTS: '#f9a825',
  HTTP_CALLS: '#7b1fa2',
  CONTAINS: '#90a4ae',
  USES: '#00838f',
  ASYNC_CALLS: '#6a1b9a',
  HANDLES_ROUTE: '#00695c',
  ACCEPTS_DTO: '#00897b',
  RETURNS_DTO: '#26a69a',
  RENDERS: '#ab47bc',
  MAPS_TO: '#ff7043',
  INJECTS: '#0277bd',
  SELECTS: '#6a1b9a',
};
const NODE_SIZES: Record<string, number> = {
  Project: 18,
  Folder: 10,
  Module: 9,
  File: 6,
  Class: 9,
  Function: 6,
  Method: 5,
  Interface: 8,
  Route: 8,
};
const NODE_LABELS = ['All', 'Project', 'Folder', 'File', 'Module', 'Class', 'Function', 'Method', 'Interface', 'Route'] as const;
const EDGE_TYPES = ['All', 'CALLS', 'USES', 'IMPORTS', 'INHERITS', 'IMPLEMENTS', 'HTTP_CALLS', 'ASYNC_CALLS', 'CONTAINS', 'HANDLES_ROUTE', 'ACCEPTS_DTO', 'RETURNS_DTO', 'RENDERS', 'MAPS_TO'] as const;

interface NeighborNode {
  id: string;
  name: string;
  label: string;
}

interface FlowRoute {
  method: string;
  path: string;
  handler: string;
  controller?: string;
}

interface FlowComponent {
  name: string;
  selector?: string;
  file_path?: string;
}

interface FlowSummary {
  layers?: { label: string; count: number }[];
  confidence?: number;
  flow_type?: string;
  renders?: number;
  injects?: number;
}

function ConfidenceCircle({ value }: { value: number }) {
  const r = 16
  const circumference = 2 * Math.PI * r
  const offset = circumference * (1 - Math.min(1, Math.max(0, value)))
  const color = value >= 0.8 ? '#22c55e' : value >= 0.5 ? '#f59e0b' : '#ef4444'
  return (
    <svg width="44" height="44" viewBox="0 0 44 44" className="shrink-0">
      <circle cx="22" cy="22" r={r} fill="none" stroke="#e5e7eb" strokeWidth="4" />
      <circle
        cx="22" cy="22" r={r}
        fill="none"
        stroke={color}
        strokeWidth="4"
        strokeDasharray={circumference}
        strokeDashoffset={offset}
        strokeLinecap="round"
        transform="rotate(-90 22 22)"
        style={{ transition: 'stroke-dashoffset 0.4s ease, stroke 0.3s' }}
      />
      <text x="22" y="26" textAnchor="middle" fontSize="10" fontWeight="700" fill="#111827">
        {Math.round(value * 100)}%
      </text>
    </svg>
  )
}

interface MinimapTransform {
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
  scale: number;
  ox: number;
  oy: number;
}

interface RawGraphEdge {
  source: string | number | { id: string | number };
  target: string | number | { id: string | number };
  type?: string;
}

interface SigmaNodeAttributes {
  id: string;
  rawId: string | number;
  x: number;
  y: number;
  label: string;   // display label (node name) — used by Sigma renderer
  nodeType: string; // node kind: "Function", "Class", etc. — used for color/size
  name: string;
  file_path?: string;
  size: number;
  color: string;
  qualified_name?: string;
}

interface SigmaEdgeAttributes {
  color: string;
  size: number;
  type: string;
  label: string;
}

function normalizeRouteList(res: unknown): FlowRoute[] {
  if (!res || typeof res !== 'object') return [];
  const o = res as Record<string, unknown>;
  if (o.error) return [];
  if (Array.isArray(o.routes)) return o.routes as FlowRoute[];
  if (Array.isArray(res)) return res as FlowRoute[];
  return [];
}

function normalizeComponentList(res: unknown): FlowComponent[] {
  if (!res || typeof res !== 'object') return [];
  const o = res as Record<string, unknown>;
  if (o.error) return [];
  if (Array.isArray(o.components)) return o.components as FlowComponent[];
  if (Array.isArray(res)) return res as FlowComponent[];
  return [];
}

const drawNodeLabel: NodeLabelDrawingFunction<SigmaNodeAttributes, SigmaEdgeAttributes, Attributes> = (
  context,
  data,
  settings,
) => {
  const text = data.label;
  if (text == null || text === '') return;
  const size = settings.labelSize;
  context.font = `${settings.labelWeight} ${size}px ${settings.labelFont}`;
  context.textAlign = 'center';
  context.textBaseline = 'bottom';
  const x = data.x;
  const y = data.y - data.size - 3;
  context.lineWidth = 3;
  context.strokeStyle = 'rgba(255,255,255,0.95)';
  context.lineJoin = 'round';
  context.strokeText(text, x, y);
  context.fillStyle = '#18181b';
  context.fillText(text, x, y);
};

function normalizeNodeId(value: string | number | { id: string | number }) {
  if (typeof value === 'object' && value !== null) return String(value.id);
  return String(value);
}

function toGraphNode(attrs: SigmaNodeAttributes): GraphNode {
  return {
    id: attrs.rawId as number,
    x: attrs.x,
    y: attrs.y,
    label: attrs.nodeType,
    name: attrs.name,
    file_path: attrs.file_path,
    size: attrs.size,
    color: attrs.color,
  } as GraphNode;
}

export default function GraphPage() {
  const [searchParams] = useSearchParams();
  const project = searchParams.get('project') || '';

  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [rawData, setRawData] = useState<GraphData | null>(null);
  const [selectedNode, setSelectedNode] = useState<GraphNode | null>(null);
  const [filteredNodeCount, setFilteredNodeCount] = useState(0);
  const [filteredEdgeCount, setFilteredEdgeCount] = useState(0);
  const [syntheticEdges, setSyntheticEdges] = useState(false);
  const [simulating, setSimulating] = useState(false);
  const [connectedCallers, setConnectedCallers] = useState<NeighborNode[]>([]);
  const [connectedCallees, setConnectedCallees] = useState<NeighborNode[]>([]);
  const [detailPos, setDetailPos] = useState<{ x: number; y: number } | null>(null);
  const [activeTab, setActiveTab] = useState<'graph' | 'flow'>('graph');
  const [searchQuery, setSearchQuery] = useState('');
  const [labelFilter, setLabelFilter] = useState('All');
  const [edgeFilter, setEdgeFilter] = useState('All');

  const [flowRoutes, setFlowRoutes] = useState<FlowRoute[]>([]);
  /** True only during initial routes/components list fetch — shows list-area spinner */
  const [flowLoading, setFlowLoading] = useState(false);
  /** True while a specific route/component flow graph is being loaded */
  const [flowGraphLoading, setFlowGraphLoading] = useState(false);
  const [flowActiveRoute, setFlowActiveRoute] = useState<FlowRoute | null>(null);
  const [flowSummary, setFlowSummary] = useState<FlowSummary | null>(null);
  const [flowMode, setFlowMode] = useState<'backend' | 'frontend'>('backend');
  const [flowComponents, setFlowComponents] = useState<FlowComponent[]>([]);
  const [flowActiveComponent, setFlowActiveComponent] = useState<FlowComponent | null>(null);

  const graphPanelRef = useRef<HTMLDivElement>(null);
  const graphContainerRef = useRef<HTMLDivElement>(null);
  const minimapCanvasRef = useRef<HTMLCanvasElement>(null);
  const sigmaRef = useRef<Sigma<SigmaNodeAttributes, SigmaEdgeAttributes, Attributes> | null>(null);
  const graphRef = useRef<MultiDirectedGraph<SigmaNodeAttributes, SigmaEdgeAttributes, Attributes> | null>(null);
  const layoutRef = useRef<FA2Layout | null>(null);
  const layoutTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const layoutStopTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const highlightedNodes = useRef<Set<string>>(new Set());
  const highlightedEdges = useRef<Set<string>>(new Set());
  const selectedNodeIdRef = useRef<string | null>(null);
  const minimapTransform = useRef<MinimapTransform | null>(null);
  const minimapDragging = useRef(false);
  const resizeObserverRef = useRef<ResizeObserver | null>(null);

  const stopLayout = useCallback(() => {
    if (layoutTimerRef.current) {
      clearInterval(layoutTimerRef.current);
      layoutTimerRef.current = null;
    }
    if (layoutStopTimerRef.current) {
      clearTimeout(layoutStopTimerRef.current);
      layoutStopTimerRef.current = null;
    }
    if (layoutRef.current) {
      try {
        layoutRef.current.stop();
      } catch {
        // best effort
      }
      try {
        layoutRef.current.kill();
      } catch {
        // best effort
      }
      layoutRef.current = null;
    }
    setSimulating(false);
  }, []);

  const killRenderer = useCallback(() => {
    stopLayout();
    if (sigmaRef.current) {
      sigmaRef.current.kill();
      sigmaRef.current = null;
    }
    graphRef.current = null;
  }, [stopLayout]);

  const drawMinimap = useCallback(() => {
    const canvas = minimapCanvasRef.current;
    const renderer = sigmaRef.current;
    const graph = graphRef.current;
    if (!canvas || !renderer || !graph) return;

    const context = canvas.getContext('2d');
    if (!context) return;

    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const width = Math.round(MINIMAP_WIDTH * dpr);
    const height = Math.round(MINIMAP_HEIGHT * dpr);

    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
      canvas.style.width = `${MINIMAP_WIDTH}px`;
      canvas.style.height = `${MINIMAP_HEIGHT}px`;
    }

    context.setTransform(1, 0, 0, 1, 0, 0);
    context.clearRect(0, 0, width, height);
    context.scale(dpr, dpr);
    context.fillStyle = '#e4e4e7';
    context.fillRect(0, 0, MINIMAP_WIDTH, MINIMAP_HEIGHT);

    const nodes = graph.nodes();
    if (!nodes.length) return;

    let minX = Infinity;
    let maxX = -Infinity;
    let minY = Infinity;
    let maxY = -Infinity;

    for (const node of nodes) {
      const attrs = graph.getNodeAttributes(node);
      if (!Number.isFinite(attrs.x) || !Number.isFinite(attrs.y)) continue;
      minX = Math.min(minX, attrs.x);
      maxX = Math.max(maxX, attrs.x);
      minY = Math.min(minY, attrs.y);
      maxY = Math.max(maxY, attrs.y);
    }

    if (!Number.isFinite(minX) || !Number.isFinite(minY)) return;

    const rangeX = Math.max(maxX - minX, 1);
    const rangeY = Math.max(maxY - minY, 1);
    const pad = 8;
    const scale = Math.min((MINIMAP_WIDTH - pad * 2) / rangeX, (MINIMAP_HEIGHT - pad * 2) / rangeY);
    const ox = (MINIMAP_WIDTH - rangeX * scale) / 2;
    const oy = (MINIMAP_HEIGHT - rangeY * scale) / 2;

    minimapTransform.current = { minX, maxX, minY, maxY, scale, ox, oy };

    const tx = (x: number) => (x - minX) * scale + ox;
    const ty = (y: number) => (y - minY) * scale + oy;

    context.globalAlpha = 0.35;
    context.strokeStyle = '#a1a1aa';
    context.lineWidth = 0.75;

    for (const edge of graph.edges()) {
      const source = graph.source(edge);
      const target = graph.target(edge);
      const sourceAttrs = graph.getNodeAttributes(source);
      const targetAttrs = graph.getNodeAttributes(target);
      context.beginPath();
      context.moveTo(tx(sourceAttrs.x), ty(sourceAttrs.y));
      context.lineTo(tx(targetAttrs.x), ty(targetAttrs.y));
      context.stroke();
    }

    context.globalAlpha = 1;
    for (const node of nodes) {
      const attrs = graph.getNodeAttributes(node);
      context.fillStyle = NODE_COLORS[attrs.nodeType] || '#888';
      context.beginPath();
      context.arc(tx(attrs.x), ty(attrs.y), 1.25, 0, Math.PI * 2);
      context.fill();
    }

    const dimensions = renderer.getDimensions();
    const topLeft = renderer.viewportToGraph({ x: 0, y: 0 });
    const bottomRight = renderer.viewportToGraph({ x: dimensions.width, y: dimensions.height });
    const viewMinX = Math.min(topLeft.x, bottomRight.x);
    const viewMaxX = Math.max(topLeft.x, bottomRight.x);
    const viewMinY = Math.min(topLeft.y, bottomRight.y);
    const viewMaxY = Math.max(topLeft.y, bottomRight.y);

    const rectX = tx(viewMinX);
    const rectY = ty(viewMinY);
    const rectW = Math.max((viewMaxX - viewMinX) * scale, 2);
    const rectH = Math.max((viewMaxY - viewMinY) * scale, 2);

    context.strokeStyle = '#15803d';
    context.lineWidth = 1.25;
    context.setLineDash([4, 3]);
    context.strokeRect(rectX + 0.5, rectY + 0.5, rectW - 1, rectH - 1);
    context.setLineDash([]);
  }, []);

  const updateDetailPanelPos = useCallback(() => {
    const renderer = sigmaRef.current;
    const graph = graphRef.current;
    const selectedId = selectedNodeIdRef.current;
    if (!renderer || !graph || !selectedId || !graph.hasNode(selectedId)) {
      setDetailPos(null);
      return;
    }

    const attrs = graph.getNodeAttributes(selectedId);
    const viewport = renderer.graphToViewport({ x: attrs.x, y: attrs.y });
    const display = renderer.getNodeDisplayData(selectedId);
    const size = display?.size ?? attrs.size;

    setDetailPos({
      x: viewport.x + size + 12,
      y: viewport.y - 10,
    });
  }, []);

  const centerCameraOnGraphPoint = useCallback((x: number, y: number, duration = 250) => {
    const renderer = sigmaRef.current;
    if (!renderer) return;

    const viewportPoint = renderer.graphToViewport({ x, y });
    const framedPoint = renderer.viewportToFramedGraph(viewportPoint);
    void renderer.getCamera().animate(
      { x: framedPoint.x, y: framedPoint.y },
      { duration },
    );
  }, []);

  const clearSelection = useCallback(() => {
    highlightedNodes.current = new Set();
    highlightedEdges.current = new Set();
    selectedNodeIdRef.current = null;
    setSelectedNode(null);
    setDetailPos(null);
    setConnectedCallers([]);
    setConnectedCallees([]);
    sigmaRef.current?.refresh();
  }, []);

  const selectNode = useCallback((id: string, center = true) => {
    const graph = graphRef.current;
    const renderer = sigmaRef.current;
    if (!graph || !renderer || !graph.hasNode(id)) return;

    const callers: NeighborNode[] = [];
    const callees: NeighborNode[] = [];
    const nextNodes = new Set<string>([id]);
    const nextEdges = new Set<string>();

    for (const edgeKey of graph.edges()) {
      const source = graph.source(edgeKey);
      const target = graph.target(edgeKey);
      if (source === id) {
        const targetAttrs = graph.getNodeAttributes(target);
        nextNodes.add(target);
        nextEdges.add(edgeKey);
        callees.push({ id: target, name: targetAttrs.name || target, label: targetAttrs.nodeType || '' });
      }
      if (target === id) {
        const sourceAttrs = graph.getNodeAttributes(source);
        nextNodes.add(source);
        nextEdges.add(edgeKey);
        callers.push({ id: source, name: sourceAttrs.name || source, label: sourceAttrs.nodeType || '' });
      }
    }

    highlightedNodes.current = nextNodes;
    highlightedEdges.current = nextEdges;
    selectedNodeIdRef.current = id;

    const attrs = graph.getNodeAttributes(id);
    setSelectedNode(toGraphNode(attrs));
    setConnectedCallers(callers);
    setConnectedCallees(callees);

    renderer.refresh();
    updateDetailPanelPos();

    if (center) centerCameraOnGraphPoint(attrs.x, attrs.y, 500);
  }, [centerCameraOnGraphPoint, updateDetailPanelPos]);

  const focusNode = useCallback((id: string) => {
    selectNode(id, true);
  }, [selectNode]);

  const buildGraph = useCallback((data: GraphData) => {
    const container = graphContainerRef.current;
    if (!container) return;

    killRenderer();
    clearSelection();
    container.innerHTML = '';

    let nodes = [...(data.nodes || [])];
    let links = [...((((data as unknown as { links?: RawGraphEdge[] }).links) || data.edges || []) as RawGraphEdge[])];

    if (labelFilter !== 'All') nodes = nodes.filter((node) => node.label === labelFilter);
    if (searchQuery) {
      const query = searchQuery.toLowerCase();
      nodes = nodes.filter((node) =>
        (node.name || '').toLowerCase().includes(query) ||
        String((node as GraphNode & { qualified_name?: string }).qualified_name || '').toLowerCase().includes(query),
      );
    }

    const nodeIds = new Set(nodes.map((node) => normalizeNodeId(node.id)));
    links = links.filter((edge) => {
      const source = normalizeNodeId(edge.source);
      const target = normalizeNodeId(edge.target);
      if (!nodeIds.has(source) || !nodeIds.has(target)) return false;
      if (edgeFilter !== 'All' && edge.type !== edgeFilter) return false;
      return true;
    });

    let hasSynthetic = false;
    if (!links.some((edge) => edge.type === 'CONTAINS')) {
      const folders = new Map<string, string>();
      for (const node of nodes) {
        if (node.label === 'Folder') folders.set(node.file_path || node.name || '', normalizeNodeId(node.id));
      }
      for (const node of nodes) {
        const nodeId = normalizeNodeId(node.id);
        if (!node.file_path || node.label === 'Folder') continue;
        const parts = node.file_path.split('/');
        parts.pop();
        const folderId = folders.get(parts.join('/'));
        if (folderId && folderId !== nodeId) {
          links.push({ source: folderId, target: nodeId, type: 'CONTAINS' });
          hasSynthetic = true;
        }
      }
    }

    setSyntheticEdges(hasSynthetic);
    setFilteredNodeCount(nodes.length);
    setFilteredEdgeCount(links.length);

    const degree = new Map<string, number>();
    for (const edge of links) {
      const source = normalizeNodeId(edge.source);
      const target = normalizeNodeId(edge.target);
      degree.set(source, (degree.get(source) || 0) + 1);
      degree.set(target, (degree.get(target) || 0) + 1);
    }
    const topLabelIds = new Set(
      nodes
        .map((node) => ({ id: normalizeNodeId(node.id), degree: degree.get(normalizeNodeId(node.id)) || 0 }))
        .sort((a, b) => b.degree - a.degree || a.id.localeCompare(b.id))
        .slice(0, 10)
        .map((item) => item.id),
    );

    const normalizedNodes = nodes.map((node, index) => {
      const id = normalizeNodeId(node.id);
      const fallbackX = Math.cos((index / Math.max(nodes.length, 1)) * Math.PI * 2);
      const fallbackY = Math.sin((index / Math.max(nodes.length, 1)) * Math.PI * 2);
      return {
        id,
        rawId: node.id,
        x: Number.isFinite(node.x) ? node.x : fallbackX,
        y: Number.isFinite(node.y) ? node.y : fallbackY,
        label: node.name || id,
        nodeType: node.label,
        name: node.name || id,
        file_path: node.file_path,
        size: NODE_SIZES[node.label] || 3,
        color: NODE_COLORS[node.label] || '#71717a',
        qualified_name: (node as GraphNode & { qualified_name?: string }).qualified_name,
      } satisfies SigmaNodeAttributes;
    });

    const allSamePosition = normalizedNodes.every(
      (node) => node.x === normalizedNodes[0]?.x && node.y === normalizedNodes[0]?.y,
    );
    if (allSamePosition) {
      normalizedNodes.forEach((node, index) => {
        node.x = Math.cos((index / Math.max(normalizedNodes.length, 1)) * Math.PI * 2);
        node.y = Math.sin((index / Math.max(normalizedNodes.length, 1)) * Math.PI * 2);
      });
    }

    const graph = new MultiDirectedGraph<SigmaNodeAttributes, SigmaEdgeAttributes, Attributes>();
    for (const node of normalizedNodes) {
      graph.addNode(node.id, node);
    }

    links.forEach((edge, index) => {
      const source = normalizeNodeId(edge.source);
      const target = normalizeNodeId(edge.target);
      if (!graph.hasNode(source) || !graph.hasNode(target)) return;
      graph.addDirectedEdgeWithKey(`${source}->${target}:${index}`, source, target, {
        color: EDGE_COLORS[edge.type || ''] || '#71717a',
        size: edge.type === 'CONTAINS' ? 0.08 : 0.15,
        type: 'line',
        label: edge.type || 'RELATED',
      });
    });

    const NodeBorderProgram = createNodeBorderProgram<SigmaNodeAttributes, SigmaEdgeAttributes, Attributes>({
      borders: [
        { size: { value: 2, mode: 'pixels' }, color: { value: 'rgba(255,255,255,0.9)' } },
        { size: { fill: true }, color: { attribute: 'color' } },
      ],
    });

    const renderer = new Sigma<SigmaNodeAttributes, SigmaEdgeAttributes, Attributes>(graph, container, {
      labelDensity: 0.6,
      labelRenderedSizeThreshold: 6,
      labelSize: 11,
      labelFont: 'Inter, system-ui, sans-serif',
      labelWeight: '500',
      defaultDrawNodeLabel: drawNodeLabel,
      defaultNodeType: 'bordered',
      nodeProgramClasses: { bordered: NodeBorderProgram },
      renderEdgeLabels: false,
      zIndex: true,
      minCameraRatio: 0.08,
      maxCameraRatio: 8,
      stagePadding: 24,
      zoomToSizeRatioFunction: (x) => x,
      nodeReducer: (node, attrs) => {
        const selected = selectedNodeIdRef.current;
        const highlighted = highlightedNodes.current;
        const shouldForceLabel = topLabelIds.has(node) || selected === node || highlighted.has(node);

        if (!selected) {
          return {
            ...attrs,
            forceLabel: shouldForceLabel,
          };
        }

        const faded = !highlighted.has(node);
        return {
          ...attrs,
          color: faded ? '#d4d4d8' : attrs.color,
          forceLabel: shouldForceLabel,
          zIndex: selected === node ? 3 : highlighted.has(node) ? 2 : 1,
        };
      },
      edgeReducer: (edge, attrs) => {
        const selected = selectedNodeIdRef.current;
        if (!selected) return attrs;
        const faded = !highlightedEdges.current.has(edge);
        return {
          ...attrs,
          color: faded ? '#e4e4e780' : attrs.color,
          size: highlightedEdges.current.has(edge) ? 1.5 : 0.08,
          zIndex: highlightedEdges.current.has(edge) ? 10 : 0,
        };
      },
    });

    graphRef.current = graph;
    sigmaRef.current = renderer;

    renderer.on('clickNode', (event: { node: string }) => {
      selectNode(String(event.node));
    });
    renderer.on('clickStage', () => {
      clearSelection();
    });

    const camera = renderer.getCamera();
    const onCameraUpdated = () => {
      updateDetailPanelPos();
      drawMinimap();
    };
    camera.on('updated', onCameraUpdated);

    const settings = forceAtlas2.inferSettings(graph);
    layoutRef.current = new FA2Layout(graph, {
      settings: {
        ...settings,
        barnesHutOptimize: graph.order > 150,
        barnesHutTheta: 0.5,
        slowDown: graph.order > 1000 ? 15 : 8,
        scalingRatio: 12,
        gravity: 0.5,
        linLogMode: true,
        outboundAttractionDistribution: true,
        adjustSizes: true,
      },
    });
    layoutRef.current.start();
    setSimulating(true);

    layoutTimerRef.current = setInterval(() => {
      sigmaRef.current?.refresh({ schedule: true });
      updateDetailPanelPos();
      drawMinimap();
    }, 180);

    layoutStopTimerRef.current = setTimeout(() => {
      stopLayout();
      sigmaRef.current?.refresh();
      updateDetailPanelPos();
      drawMinimap();
    }, 5000);

    renderer.refresh();
    void renderer.getCamera().animatedReset({ duration: 0 });
    drawMinimap();
  }, [clearSelection, drawMinimap, edgeFilter, killRenderer, labelFilter, searchQuery, selectNode, stopLayout, updateDetailPanelPos]);

  const loadGraph = useCallback(async () => {
    if (!project) return;
    setLoading(true);
    setLoadError(null);
    try {
      const data = await fetchLayout(project);
      setRawData(data);
      buildGraph(data);
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Failed to load graph';
      setLoadError(message);
    } finally {
      setLoading(false);
    }
  }, [buildGraph, project]);

  const loadRoutes = useCallback(async () => {
    if (!project) return;
    setFlowLoading(true);
    try {
      const [routesRes, compsRes] = await Promise.all([fetchRoutes(project), fetchComponents(project)]);
      setFlowRoutes(normalizeRouteList(routesRes));
      setFlowComponents(normalizeComponentList(compsRes));
    } catch {
      // leave current state untouched
    } finally {
      setFlowLoading(false);
    }
  }, [project]);

  const loadBackendFlow = useCallback(async (route: FlowRoute) => {
    if (!project) return;
    setFlowActiveRoute(route);
    setFlowActiveComponent(null);
    setFlowGraphLoading(true);
    try {
      const flow = await fetchBackendFlow(project, route.path, route.method);
      const nodes: GraphNode[] = [];
      const links: RawGraphEdge[] = [];
      if (Array.isArray(flow?.nodes)) {
        for (const node of flow.nodes) {
          nodes.push({
            id: (node.id ?? node.name) as number,
            x: 0,
            y: 0,
            name: node.name,
            label: node.label || 'Function',
            file_path: node.file_path,
            size: node.size ?? (NODE_SIZES[node.label || 'Function'] || 3),
            color: node.color ?? (NODE_COLORS[node.label || 'Function'] || '#71717a'),
          });
        }
      }
      if (Array.isArray(flow?.edges)) {
        for (const edge of flow.edges) {
          links.push({ source: edge.source, target: edge.target, type: edge.type || 'CALLS' });
        }
      }
      if (flow?.summary) setFlowSummary(flow.summary);
      buildGraph({ nodes, edges: links as GraphData['edges'], total_nodes: nodes.length });
    } catch {
      // keep previous flow graph if the request fails
    } finally {
      setFlowGraphLoading(false);
    }
  }, [buildGraph, project]);

  const loadFrontendFlow = useCallback(async (component: FlowComponent) => {
    if (!project) return;
    setFlowActiveComponent(component);
    setFlowActiveRoute(null);
    setFlowGraphLoading(true);
    try {
      const flow = await fetchFrontendFlow(project, component.name);
      const nodes: GraphNode[] = [];
      const links: RawGraphEdge[] = [];
      if (Array.isArray(flow?.nodes)) {
        for (const node of flow.nodes) {
          nodes.push({
            id: (node.id || node.name) as number,
            x: 0,
            y: 0,
            name: node.name,
            label: node.label || 'Class',
            file_path: node.file_path,
            size: NODE_SIZES[node.label || 'Class'] || 3,
            color: NODE_COLORS[node.label || 'Class'] || '#71717a',
          });
        }
      }
      if (Array.isArray(flow?.edges)) {
        for (const edge of flow.edges) {
          links.push({
            source: edge.source,
            target: edge.target,
            type: edge.type || 'RENDERS',
          });
        }
      }
      if (flow?.summary) setFlowSummary(flow.summary);
      buildGraph({ nodes, edges: links as GraphData['edges'], total_nodes: nodes.length });
    } catch {
      // keep previous flow graph if the request fails
    } finally {
      setFlowGraphLoading(false);
    }
  }, [buildGraph, project]);

  useEffect(() => {
    loadGraph();
  }, [loadGraph]);

  useEffect(() => {
    if (activeTab === 'flow') loadRoutes();
  }, [activeTab, loadRoutes]);

  useEffect(() => {
    if (rawData && activeTab === 'graph') buildGraph(rawData);
  }, [activeTab, buildGraph, edgeFilter, labelFilter, rawData, searchQuery]);

  useEffect(() => {
    const element = graphContainerRef.current;
    if (!element) return;
    resizeObserverRef.current = new ResizeObserver(() => {
      sigmaRef.current?.resize(true);
      updateDetailPanelPos();
      drawMinimap();
    });
    resizeObserverRef.current.observe(element);
    return () => {
      resizeObserverRef.current?.disconnect();
    };
  }, [drawMinimap, updateDetailPanelPos]);

  useEffect(() => {
    const endDrag = () => {
      minimapDragging.current = false;
    };
    window.addEventListener('mouseup', endDrag);
    return () => {
      window.removeEventListener('mouseup', endDrag);
    };
  }, []);

  useEffect(() => () => {
    killRenderer();
  }, [killRenderer]);

  const moveCameraFromMinimap = useCallback((event: React.MouseEvent<HTMLCanvasElement>) => {
    const transform = minimapTransform.current;
    const canvas = minimapCanvasRef.current;
    if (!transform || !canvas) return;
    const rect = canvas.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;
    const graphX = (x - transform.ox) / transform.scale + transform.minX;
    const graphY = (y - transform.oy) / transform.scale + transform.minY;
    centerCameraOnGraphPoint(graphX, graphY, 180);
  }, [centerCameraOnGraphPoint]);

  const zoomIn = () => {
    void sigmaRef.current?.getCamera().animatedZoom({ duration: 180 });
  };
  const zoomOut = () => {
    void sigmaRef.current?.getCamera().animatedUnzoom({ duration: 180 });
  };
  const zoomFit = () => {
    void sigmaRef.current?.getCamera().animatedReset({ duration: 250 });
  };

  if (!project) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-zinc-100 text-zinc-600">
        <div className="text-center space-y-2">
          <Box className="w-12 h-12 mx-auto text-zinc-400" />
          <p className="text-lg text-zinc-900">No project selected</p>
          <p className="text-sm">Add <code className="bg-zinc-200 text-zinc-800 px-1.5 py-0.5 rounded text-sm">?project=name</code> to the URL</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden bg-zinc-100 text-zinc-900">
      {/* Top bar */}
      <div className="flex items-center gap-3 px-4 py-2 border-b border-zinc-200 bg-white shrink-0">
        <Badge variant="outline" className="text-sm font-mono border-zinc-300">{project}</Badge>
        <span className="text-xs text-zinc-600">{filteredNodeCount} nodes · {filteredEdgeCount} edges</span>
        {syntheticEdges && <Badge variant="secondary" className="text-[10px]">synthetic CONTAINS</Badge>}
        {simulating && <Badge variant="secondary" className="text-[10px] animate-pulse">simulating…</Badge>}
      </div>

      <div className="flex min-h-0 flex-1 overflow-hidden">
        {/* Sidebar */}
        <div className="w-[260px] shrink-0 border-r border-zinc-200 bg-white flex flex-col overflow-hidden">
          {/* Tab buttons */}
          <div className="flex border-b border-zinc-200">
            <button onClick={() => setActiveTab('graph')} className={`flex-1 py-2 text-xs font-medium transition-colors ${activeTab === 'graph' ? 'bg-zinc-100 text-zinc-900' : 'text-zinc-500 hover:text-zinc-800 hover:bg-zinc-50'}`}>
              <GitBranch className="w-3 h-3 inline mr-1" />Graph
            </button>
            <button onClick={() => setActiveTab('flow')} className={`flex-1 py-2 text-xs font-medium transition-colors ${activeTab === 'flow' ? 'bg-zinc-100 text-zinc-900' : 'text-zinc-500 hover:text-zinc-800 hover:bg-zinc-50'}`}>
              <ArrowRight className="w-3 h-3 inline mr-1" />Flow
            </button>
          </div>

          <div className="flex-1 overflow-y-auto p-3 space-y-3">
            {activeTab === 'graph' ? (
              <>
                {/* Search */}
                <div className="relative">
                  <Search className="absolute left-2 top-2 w-3.5 h-3.5 text-zinc-400" />
                  <Input placeholder="Search nodes…" value={searchQuery} onChange={e => setSearchQuery(e.target.value)} className="pl-7 h-8 text-xs bg-white border-zinc-300" />
                </div>

                {/* Label filter */}
                <div className="flex flex-col gap-1">
                  <label className="text-[10px] font-medium text-zinc-500 uppercase tracking-wide px-0.5">Node type</label>
                  <Select value={labelFilter} onValueChange={v => setLabelFilter(v ?? 'All')}>
                    <SelectTrigger className="w-full h-8 text-xs bg-white border-zinc-300"><SelectValue /></SelectTrigger>
                    <SelectContent>{NODE_LABELS.map(l => <SelectItem key={l} value={l} className="text-xs">{l === 'All' ? 'All node types' : l}</SelectItem>)}</SelectContent>
                  </Select>
                </div>

                {/* Edge filter */}
                <div className="flex flex-col gap-1">
                  <label className="text-[10px] font-medium text-zinc-500 uppercase tracking-wide px-0.5">Edge type</label>
                  <Select value={edgeFilter} onValueChange={v => setEdgeFilter(v ?? 'All')}>
                    <SelectTrigger className="w-full h-8 text-xs bg-white border-zinc-300"><SelectValue /></SelectTrigger>
                    <SelectContent>{EDGE_TYPES.map(t => <SelectItem key={t} value={t} className="text-xs">{t === 'All' ? 'All edge types' : t}</SelectItem>)}</SelectContent>
                  </Select>
                </div>

                {/* Node legend */}
                <div className="space-y-1">
                  <p className="text-[10px] text-zinc-500 uppercase tracking-wider">Nodes</p>
                  <div className="flex flex-wrap gap-x-3 gap-y-1">
                    {Object.entries(NODE_COLORS).map(([k, c]) => (
                      <span key={k} className="flex items-center gap-1 text-[10px] text-zinc-700">
                        <span className="w-2 h-2 rounded-full inline-block" style={{ backgroundColor: c }} />{k}
                      </span>
                    ))}
                  </div>
                </div>

                {/* Edge legend */}
                <div className="space-y-1">
                  <p className="text-[10px] text-zinc-500 uppercase tracking-wider">Edges</p>
                  <div className="flex flex-wrap gap-x-3 gap-y-1">
                    {Object.entries(EDGE_COLORS).map(([k, c]) => (
                      <span key={k} className="flex items-center gap-1 text-[10px] text-zinc-700">
                        <span className="w-3 h-0.5 inline-block" style={{ backgroundColor: c }} />{k}
                      </span>
                    ))}
                  </div>
                </div>
              </>
            ) : (
              /* Flow tab */
              <>
                <div className="flex gap-1">
                  <Button size="sm" variant={flowMode === 'backend' ? 'default' : 'ghost'} className="flex-1 h-7 text-xs" onClick={() => setFlowMode('backend')}>Routes</Button>
                  <Button size="sm" variant={flowMode === 'frontend' ? 'default' : 'ghost'} className="flex-1 h-7 text-xs" onClick={() => setFlowMode('frontend')}>Components</Button>
                </div>

                {flowMode === 'backend' ? (
                  <div className="relative space-y-1">
                    {flowLoading && (
                      <div className="absolute inset-0 flex items-center justify-center bg-white/70 z-10 rounded">
                        <Loader2 className="w-4 h-4 animate-spin text-zinc-400" />
                      </div>
                    )}
                    {flowRoutes.map((r, i) => (
                      <button key={i} onClick={() => loadBackendFlow(r)} className={`w-full text-left p-2 rounded text-xs hover:bg-zinc-100 ${flowActiveRoute === r ? 'bg-zinc-100 ring-1 ring-zinc-300' : ''}`}>
                        <Badge variant="outline" className="text-[9px] mr-1.5" style={{ color: r.method === 'GET' ? '#4caf50' : r.method === 'POST' ? '#2196f3' : r.method === 'DELETE' ? '#f44336' : '#ff9800' }}>{r.method}</Badge>
                        <span className="text-zinc-800 font-mono">{r.path}</span>
                        {r.handler && <p className="text-[10px] text-zinc-500 mt-0.5 truncate">{r.handler}</p>}
                      </button>
                    ))}
                    {!flowLoading && !flowRoutes.length && <p className="text-xs text-zinc-500 text-center py-4">No routes found</p>}
                  </div>
                ) : (
                  <div className="relative space-y-1">
                    {flowLoading && (
                      <div className="absolute inset-0 flex items-center justify-center bg-white/70 z-10 rounded">
                        <Loader2 className="w-4 h-4 animate-spin text-zinc-400" />
                      </div>
                    )}
                    {flowComponents.map((c, i) => (
                      <button key={i} onClick={() => loadFrontendFlow(c)} className={`w-full text-left p-2 rounded text-xs hover:bg-zinc-100 ${flowActiveComponent === c ? 'bg-zinc-100 ring-1 ring-zinc-300' : ''}`}>
                        <span className="text-zinc-800">{c.name}</span>
                        {c.selector && <p className="text-[10px] text-zinc-500 font-mono">{c.selector}</p>}
                      </button>
                    ))}
                    {!flowLoading && !flowComponents.length && <p className="text-xs text-zinc-500 text-center py-4">No components found</p>}
                  </div>
                )}

              </>
            )}
          </div>

          {/* Flow Summary — pinned at bottom of sidebar, always visible on Flow tab */}
          {activeTab === 'flow' && (
            <div className="shrink-0 border-t border-zinc-200 bg-white px-3 py-2.5">
              <div className="flex items-center gap-1.5 mb-2">
                <p className="text-[10px] font-semibold uppercase tracking-wider text-zinc-400">Flow Summary</p>
                {flowGraphLoading && <Loader2 className="w-3 h-3 animate-spin text-zinc-400" />}
              </div>
              {flowSummary && !flowGraphLoading ? (
                <div className="flex gap-2.5 items-start">
                  {typeof flowSummary.confidence === 'number' && (
                    <ConfidenceCircle value={flowSummary.confidence} />
                  )}
                  <div className="flex-1 space-y-1 min-w-0 pt-0.5">
                    {flowSummary.flow_type != null && flowSummary.flow_type !== '' && (
                      <div className="flex justify-between text-xs gap-2">
                        <span className="text-zinc-500 shrink-0">Type</span>
                        <span className="text-zinc-800 font-mono truncate">{flowSummary.flow_type}</span>
                      </div>
                    )}
                    {Array.isArray(flowSummary.layers) && flowSummary.layers.length > 0 && flowSummary.layers.map((l, i) => (
                      <div key={i} className="flex justify-between text-xs">
                        <span className="text-zinc-500">{l.label}</span>
                        <span className="text-zinc-800">{l.count}</span>
                      </div>
                    ))}
                    {typeof flowSummary.renders === 'number' && (
                      <div className="flex justify-between text-xs">
                        <span className="text-zinc-500">Renders</span>
                        <span className="text-zinc-800">{flowSummary.renders}</span>
                      </div>
                    )}
                    {typeof flowSummary.injects === 'number' && (
                      <div className="flex justify-between text-xs">
                        <span className="text-zinc-500">Injects</span>
                        <span className="text-zinc-800">{flowSummary.injects}</span>
                      </div>
                    )}
                  </div>
                </div>
              ) : !flowGraphLoading ? (
                <p className="text-[11px] text-zinc-400 text-center py-1">Select a route or component</p>
              ) : null}
            </div>
          )}

          <p className="text-[10px] text-zinc-500 px-3 py-2 border-t border-zinc-200">Click a node to inspect · Scroll to zoom</p>
        </div>

        {/* Graph canvas area */}
        <div
          ref={graphPanelRef}
          className="relative flex min-h-0 flex-1 overflow-hidden bg-zinc-100"
          style={{
            backgroundImage:
              'radial-gradient(circle at 1px 1px, rgba(161,161,170,0.55) 1px, transparent 0)',
            backgroundSize: '30px 30px',
          }}
        >
          <div ref={graphContainerRef} className="h-full min-h-0 w-full min-w-0" />

          {/* Loading overlay */}
          {loading && (
            <div className="absolute inset-0 flex items-center justify-center bg-white/85 backdrop-blur-[2px] z-20">
              <div className="flex flex-col items-center gap-2">
                <Loader2 className="w-8 h-8 animate-spin text-blue-600" />
                <span className="text-sm text-zinc-600">Loading graph…</span>
              </div>
            </div>
          )}

          {/* Error overlay */}
          {loadError && (
            <div className="absolute inset-0 flex items-center justify-center bg-white/85 backdrop-blur-[2px] z-20">
              <div className="flex flex-col items-center gap-2 text-center max-w-sm">
                <AlertCircle className="w-8 h-8 text-red-600" />
                <p className="text-sm text-red-700">{loadError}</p>
                <Button size="sm" variant="outline" onClick={loadGraph}>Retry</Button>
              </div>
            </div>
          )}

          {/* Node detail popover */}
          {selectedNode && detailPos && (
            <div
              className="absolute z-30 w-64 bg-white border border-zinc-200 rounded-lg shadow-xl"
              style={{
                left: Math.max(8, Math.min(detailPos.x, (graphPanelRef.current?.clientWidth || 800) - DETAIL_PANEL_WIDTH)),
                top: Math.max(8, Math.min(detailPos.y, (graphPanelRef.current?.clientHeight || 600) - DETAIL_PANEL_HEIGHT)),
              }}
            >
              <div className="flex items-center gap-2 p-2.5 border-b border-zinc-200">
                <MapPin className="w-3.5 h-3.5 text-zinc-400 shrink-0" />
                <span className="text-xs font-medium truncate flex-1 text-zinc-900">{selectedNode.name || selectedNode.id}</span>
                <button onClick={clearSelection} className="text-zinc-400 hover:text-zinc-700"><X className="w-3.5 h-3.5" /></button>
              </div>
              <div className="p-2.5 space-y-2 text-xs">
                <div className="flex items-center gap-2">
                  <Badge style={{ backgroundColor: NODE_COLORS[selectedNode.label] || '#888' }} className="text-[10px] text-white">{selectedNode.label}</Badge>
                  {selectedNode.file_path && <span className="text-zinc-600 truncate font-mono text-[10px]">{selectedNode.file_path}</span>}
                </div>
                <p className="text-[10px] text-zinc-500 font-mono truncate">{selectedNode.id}</p>

                {connectedCallers.length > 0 && (
                  <div>
                    <p className="text-[10px] text-zinc-500 mb-1">Callers ({connectedCallers.length})</p>
                    <div className="max-h-24 overflow-y-auto space-y-0.5">
                      {connectedCallers.map(c => (
                        <button key={c.id} onClick={() => focusNode(c.id)} className="block w-full text-left px-1.5 py-0.5 rounded hover:bg-zinc-100 text-[11px] text-zinc-800 truncate">
                          <span className="w-1.5 h-1.5 rounded-full inline-block mr-1" style={{ backgroundColor: NODE_COLORS[c.label] || '#888' }} />{c.name}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {connectedCallees.length > 0 && (
                  <div>
                    <p className="text-[10px] text-zinc-500 mb-1">Callees ({connectedCallees.length})</p>
                    <div className="max-h-24 overflow-y-auto space-y-0.5">
                      {connectedCallees.map(c => (
                        <button key={c.id} onClick={() => focusNode(c.id)} className="block w-full text-left px-1.5 py-0.5 rounded hover:bg-zinc-100 text-[11px] text-zinc-800 truncate">
                          <span className="w-1.5 h-1.5 rounded-full inline-block mr-1" style={{ backgroundColor: NODE_COLORS[c.label] || '#888' }} />{c.name}
                        </button>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          )}

          {/* Zoom row above minimap */}
          <div className="pointer-events-auto absolute bottom-3 right-3 z-10 flex flex-col items-end gap-2">
            <div className="flex flex-row gap-1">
              <Button size="icon" variant="outline" className="h-8 w-8 border-zinc-300 bg-white/95 shadow-sm" onClick={zoomIn}><Plus className="h-4 w-4" /></Button>
              <Button size="icon" variant="outline" className="h-8 w-8 border-zinc-300 bg-white/95 shadow-sm" onClick={zoomOut}><Minus className="h-4 w-4" /></Button>
              <Button size="icon" variant="outline" className="h-8 w-8 border-zinc-300 bg-white/95 shadow-sm" onClick={zoomFit}><Maximize className="h-4 w-4" /></Button>
            </div>
            <div className="overflow-hidden rounded-md border border-zinc-300 bg-zinc-200/80 shadow-sm">
              <canvas
                ref={minimapCanvasRef}
                className="block cursor-crosshair"
                aria-label="Graph overview"
                onMouseDown={(event) => {
                  minimapDragging.current = true;
                  moveCameraFromMinimap(event);
                }}
                onMouseMove={(event) => {
                  if (minimapDragging.current) moveCameraFromMinimap(event);
                }}
                onMouseLeave={() => {
                  minimapDragging.current = false;
                }}
              />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
