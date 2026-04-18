# Graph Page Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `force-graph` (Canvas 2D) with sigma.js v3 + graphology (WebGL), fix layout scroll, add interactive minimap, and make the node detail panel track its node as the camera moves.

**Architecture:** `GraphPage.tsx` is a full rewrite keeping all state shape, constants, and sidebar/flow-tab logic identical. The core change is: graphology `MultiDirectedGraph` replaces the raw node/link arrays; `Sigma` renders it via WebGL; `FA2Layout` worker runs layout off the main thread. Minimap is a standalone `<canvas>` element updated on sigma camera events with click/drag navigation.

**Tech Stack:** sigma@3.0.2, graphology@0.26.0, graphology-layout-forceatlas2@0.10.1, React 19, TypeScript, Tailwind CSS v4

---

### Task 1: Install dependencies

**Files:**
- Modify: `ui/package.json`

- [ ] **Step 1: Remove force-graph, add sigma packages**

```bash
cd ui
npm remove force-graph
npm install sigma@3.0.2 graphology@0.26.0 graphology-layout-forceatlas2@0.10.1
```

- [ ] **Step 2: Verify install**

```bash
npm ls sigma graphology graphology-layout-forceatlas2
```

Expected: all three listed with correct versions, no peer-dep errors.

- [ ] **Step 3: Verify TypeScript sees them**

```bash
npx tsc --noEmit 2>&1 | head -20
```

Expected: errors about `force-graph` imports in `GraphPage.tsx` (those get fixed in Task 3), but NO errors about missing sigma/graphology types.

- [ ] **Step 4: Commit**

```bash
git add ui/package.json ui/package-lock.json
git commit -m "chore: replace force-graph with sigma + graphology stack"
```

---

### Task 2: Fix layout scroll

**Files:**
- Modify: `ui/src/App.tsx`
- Modify: `ui/src/index.css`

- [ ] **Step 1: Fix App.tsx root div and main padding**

Replace the root `<div>` and `<main>` in `App.tsx`:

```tsx
// Before:
<div className="flex min-h-screen flex-col bg-[#fafbfc]">
  ...
  <main
    className={cn(
      'mx-auto flex w-full max-w-[1440px] min-h-0 flex-1 flex-col px-6 py-6',
      graphRoute ? 'overflow-hidden bg-zinc-100' : 'bg-[#fafbfc]',
    )}
  >

// After:
<div className="flex h-screen overflow-hidden flex-col bg-[#fafbfc]">
  ...
  <main
    className={cn(
      'mx-auto flex w-full max-w-[1440px] min-h-0 flex-1 flex-col',
      graphRoute
        ? 'overflow-hidden bg-zinc-100'
        : 'px-6 py-6 bg-[#fafbfc]',
    )}
  >
```

- [ ] **Step 2: Pin html and body height in index.css**

Add after the existing `body { ... }` block:

```css
html, body {
  height: 100%;
  overflow: hidden;
}
```

- [ ] **Step 3: Verify build compiles**

```bash
cd ui && npm run build 2>&1 | tail -10
```

Expected: build succeeds (GraphPage still has force-graph import errors — that is fine, we'll fix in Task 3).

- [ ] **Step 4: Commit**

```bash
git add ui/src/App.tsx ui/src/index.css
git commit -m "fix: eliminate graph page scroll with h-screen and no padding for graph routes"
```

---

### Task 3: Rewrite GraphPage — imports, constants, state, skeleton

**Files:**
- Modify: `ui/src/pages/GraphPage.tsx` (full rewrite begins here)

Replace the entire file with the skeleton below. This compiles but shows an empty canvas — rendering comes in Task 4.

- [ ] **Step 1: Write the new GraphPage.tsx skeleton**

```tsx
import { useCallback, useEffect, useRef, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import Sigma from 'sigma';
import { MultiDirectedGraph } from 'graphology';
import FA2Layout from 'graphology-layout-forceatlas2/worker';
import { Plus, Minus, Maximize, MapPin, X, Loader2, AlertCircle, Search, GitBranch, Box, ArrowRight } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Badge } from '@/components/ui/badge';
import { fetchLayout, fetchRoutes, fetchBackendFlow, fetchComponents, fetchFrontendFlow } from '@/lib/rpc';
import type { GraphData, GraphNode } from '@/lib/types';

// ── Constants (identical to original) ──────────────────────────────────────
const NODE_COLORS: Record<string, string> = {
  Project: '#c62828', Folder: '#37474f', File: '#546e7a', Module: '#7b1fa2',
  Class: '#e64a19', Function: '#1976d2', Method: '#388e3c', Interface: '#f9a825',
  Route: '#00695c', Selector: '#ab47bc',
};
const EDGE_COLORS: Record<string, string> = {
  CALLS: '#1976d2', IMPORTS: '#388e3c', INHERITS: '#e64a19', IMPLEMENTS: '#f9a825',
  HTTP_CALLS: '#7b1fa2', CONTAINS: '#90a4ae', USES: '#00838f', ASYNC_CALLS: '#6a1b9a',
  HANDLES_ROUTE: '#00695c', ACCEPTS_DTO: '#00897b', RETURNS_DTO: '#26a69a',
  RENDERS: '#ab47bc', MAPS_TO: '#ff7043', INJECTS: '#0277bd', SELECTS: '#6a1b9a',
};
const NODE_SIZES: Record<string, number> = {
  Project: 12, Folder: 6, Module: 5, File: 3, Class: 5,
  Function: 3, Method: 2, Interface: 4, Route: 4,
};
const NODE_LABELS = ['All', 'Project', 'Folder', 'File', 'Module', 'Class', 'Function', 'Method', 'Interface', 'Route'] as const;
const EDGE_TYPES = ['All', 'CALLS', 'USES', 'IMPORTS', 'INHERITS', 'IMPLEMENTS', 'HTTP_CALLS', 'ASYNC_CALLS', 'CONTAINS', 'HANDLES_ROUTE', 'ACCEPTS_DTO', 'RETURNS_DTO', 'RENDERS', 'MAPS_TO'] as const;

// ── Neighbor types ──────────────────────────────────────────────────────────
interface NeighborNode { id: string; name: string; label: string }
interface FlowRoute { method: string; path: string; handler: string; controller?: string }
interface FlowComponent { name: string; selector?: string; file_path?: string }
interface FlowSummary {
  layers?: { label: string; count: number }[];
  confidence?: number; flow_type?: string; renders?: number; injects?: number;
}

// ── Minimap transform stored between draws ──────────────────────────────────
interface MinimapTransform {
  minX: number; maxX: number; minY: number; maxY: number;
  scale: number; ox: number; oy: number;
  normRangeX: number; normRangeY: number;
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

export default function GraphPage() {
  const [searchParams] = useSearchParams();
  const project = searchParams.get('project') || '';

  // ── Graph state ──────────────────────────────────────────────────────────
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

  // ── Flow state ───────────────────────────────────────────────────────────
  const [flowRoutes, setFlowRoutes] = useState<FlowRoute[]>([]);
  const [flowLoading, setFlowLoading] = useState(false);
  const [flowActiveRoute, setFlowActiveRoute] = useState<FlowRoute | null>(null);
  const [flowSummary, setFlowSummary] = useState<FlowSummary | null>(null);
  const [flowMode, setFlowMode] = useState<'backend' | 'frontend'>('backend');
  const [flowComponents, setFlowComponents] = useState<FlowComponent[]>([]);
  const [flowActiveComponent, setFlowActiveComponent] = useState<FlowComponent | null>(null);

  // ── Refs ─────────────────────────────────────────────────────────────────
  const graphPanelRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const minimapCanvasRef = useRef<HTMLCanvasElement>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  const graphRef = useRef<MultiDirectedGraph | null>(null);
  const layoutRef = useRef<FA2Layout | null>(null);
  const layoutTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const highlightedNodes = useRef<Set<string>>(new Set());
  const highlightedEdges = useRef<Set<string>>(new Set());
  const selectedNodeIdRef = useRef<string | null>(null);
  const minimapTransform = useRef<MinimapTransform | null>(null);
  const minimapDragging = useRef(false);
  const resizeObserverRef = useRef<ResizeObserver | null>(null);

  // ── Placeholder stubs (filled in subsequent tasks) ───────────────────────
  const drawMinimap = useCallback(() => { /* Task 7 */ }, []);
  const updateDetailPanelPos = useCallback(() => { /* Task 6 */ }, []);

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

  const buildGraph = useCallback((_data: GraphData) => {
    // Task 4 — dep array will be updated in Task 5 to include selectNode
  }, [labelFilter, searchQuery, edgeFilter, clearSelection, drawMinimap, updateDetailPanelPos]);

  const loadGraph = useCallback(async () => {
    if (!project) return;
    setLoading(true);
    setLoadError(null);
    try {
      const data = await fetchLayout(project);
      setRawData(data);
      buildGraph(data);
    } catch (e: any) {
      setLoadError(e.message || 'Failed to load graph');
    } finally {
      setLoading(false);
    }
  }, [project, buildGraph]);

  useEffect(() => { loadGraph(); }, [loadGraph]);

  useEffect(() => {
    if (rawData && activeTab === 'graph') buildGraph(rawData);
  }, [labelFilter, searchQuery, edgeFilter, rawData, activeTab, buildGraph]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    resizeObserverRef.current = new ResizeObserver(() => {
      sigmaRef.current?.resize();
      drawMinimap();
    });
    resizeObserverRef.current.observe(el);
    return () => resizeObserverRef.current?.disconnect();
  }, [drawMinimap]);

  useEffect(() => () => {
    layoutTimerRef.current && clearInterval(layoutTimerRef.current);
    layoutRef.current?.kill();
    sigmaRef.current?.kill();
  }, []);

  const zoomIn = () => {
    const c = sigmaRef.current?.getCamera();
    if (c) c.animate({ ratio: c.getState().ratio / 1.4 }, { duration: 300 });
  };
  const zoomOut = () => {
    const c = sigmaRef.current?.getCamera();
    if (c) c.animate({ ratio: c.getState().ratio * 1.4 }, { duration: 300 });
  };
  const zoomFit = () => sigmaRef.current?.getCamera().animate({ x: 0.5, y: 0.5, ratio: 1 }, { duration: 400 });

  // ── Flow loaders (unchanged logic, call buildGraph at end) ───────────────
  const loadRoutes = useCallback(async () => {
    if (!project) return;
    setFlowLoading(true);
    try {
      const [routesRes, compsRes] = await Promise.all([fetchRoutes(project), fetchComponents(project)]);
      setFlowRoutes(normalizeRouteList(routesRes));
      setFlowComponents(normalizeComponentList(compsRes));
    } catch {} finally { setFlowLoading(false); }
  }, [project]);

  const loadBackendFlow = useCallback(async (route: FlowRoute) => {
    if (!project) return;
    setFlowActiveRoute(route); setFlowActiveComponent(null); setFlowLoading(true);
    try {
      const flow = await fetchBackendFlow(project, route.path, route.method);
      const nodes: GraphNode[] = [];
      const links: any[] = [];
      if (flow?.steps) {
        for (const step of flow.steps)
          nodes.push({ id: step.id || step.name, name: step.name, label: step.label || 'Function', file_path: step.file_path } as GraphNode);
        for (let i = 0; i < flow.steps.length - 1; i++)
          links.push({ source: flow.steps[i].id || flow.steps[i].name, target: flow.steps[i+1].id || flow.steps[i+1].name, type: 'CALLS' });
      }
      if (flow?.summary) setFlowSummary(flow.summary);
      buildGraph({ nodes, edges: links, total_nodes: nodes.length } as unknown as GraphData);
    } catch {} finally { setFlowLoading(false); }
  }, [project, buildGraph]);

  const loadFrontendFlow = useCallback(async (comp: FlowComponent) => {
    if (!project) return;
    setFlowActiveComponent(comp); setFlowActiveRoute(null); setFlowLoading(true);
    try {
      const flow = await fetchFrontendFlow(project, comp.name);
      const nodes: GraphNode[] = [];
      const links: any[] = [];
      if (flow?.nodes)
        for (const n of flow.nodes) nodes.push({ id: n.id || n.name, name: n.name, label: n.label || 'Class', file_path: n.file_path } as GraphNode);
      if (flow?.edges)
        for (const e of flow.edges) links.push({ source: e.source, target: e.target, type: e.type || 'RENDERS' });
      if (flow?.summary) setFlowSummary(flow.summary);
      buildGraph({ nodes, edges: links, total_nodes: nodes.length } as unknown as GraphData);
    } catch {} finally { setFlowLoading(false); }
  }, [project, buildGraph]);

  useEffect(() => { if (activeTab === 'flow') loadRoutes(); }, [activeTab, loadRoutes]);

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
                <div className="relative">
                  <Search className="absolute left-2 top-2 w-3.5 h-3.5 text-zinc-400" />
                  <Input placeholder="Search nodes…" value={searchQuery} onChange={e => setSearchQuery(e.target.value)} className="pl-7 h-8 text-xs bg-white border-zinc-300" />
                </div>
                <Select value={labelFilter} onValueChange={v => setLabelFilter(v ?? 'All')}>
                  <SelectTrigger className="h-8 text-xs bg-white border-zinc-300"><SelectValue /></SelectTrigger>
                  <SelectContent>{NODE_LABELS.map(l => <SelectItem key={l} value={l} className="text-xs">{l === 'All' ? 'All node types' : l}</SelectItem>)}</SelectContent>
                </Select>
                <Select value={edgeFilter} onValueChange={v => setEdgeFilter(v ?? 'All')}>
                  <SelectTrigger className="h-8 text-xs bg-white border-zinc-300"><SelectValue /></SelectTrigger>
                  <SelectContent>{EDGE_TYPES.map(t => <SelectItem key={t} value={t} className="text-xs">{t === 'All' ? 'All edge types' : t}</SelectItem>)}</SelectContent>
                </Select>
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
              <>
                <div className="flex gap-1">
                  <Button size="sm" variant={flowMode === 'backend' ? 'default' : 'ghost'} className="flex-1 h-7 text-xs" onClick={() => setFlowMode('backend')}>Routes</Button>
                  <Button size="sm" variant={flowMode === 'frontend' ? 'default' : 'ghost'} className="flex-1 h-7 text-xs" onClick={() => setFlowMode('frontend')}>Components</Button>
                </div>
                {flowLoading && <div className="flex justify-center py-4"><Loader2 className="w-4 h-4 animate-spin text-zinc-400" /></div>}
                {flowMode === 'backend' ? (
                  <div className="space-y-1">
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
                  <div className="space-y-1">
                    {flowComponents.map((c, i) => (
                      <button key={i} onClick={() => loadFrontendFlow(c)} className={`w-full text-left p-2 rounded text-xs hover:bg-zinc-100 ${flowActiveComponent === c ? 'bg-zinc-100 ring-1 ring-zinc-300' : ''}`}>
                        <span className="text-zinc-800">{c.name}</span>
                        {c.selector && <p className="text-[10px] text-zinc-500 font-mono">{c.selector}</p>}
                      </button>
                    ))}
                    {!flowLoading && !flowComponents.length && <p className="text-xs text-zinc-500 text-center py-4">No components found</p>}
                  </div>
                )}
                {flowSummary && (
                  <div className="bg-zinc-100 rounded-md p-2 space-y-1 border border-zinc-200">
                    <p className="text-[10px] text-zinc-500 uppercase tracking-wider">Flow Summary</p>
                    {Array.isArray(flowSummary.layers) && flowSummary.layers.length > 0 ? (
                      flowSummary.layers.map((l, i) => (
                        <div key={i} className="flex justify-between text-xs">
                          <span className="text-zinc-700">{l.label}</span><span className="text-zinc-600">{l.count}</span>
                        </div>
                      ))
                    ) : (
                      <>
                        {flowSummary.flow_type && <div className="flex justify-between text-xs gap-2"><span className="text-zinc-700 shrink-0">Type</span><span className="text-zinc-600 font-mono truncate">{flowSummary.flow_type}</span></div>}
                        {typeof flowSummary.confidence === 'number' && <div className="flex justify-between text-xs"><span className="text-zinc-700">Confidence</span><span className="text-zinc-600">{(flowSummary.confidence * 100).toFixed(0)}%</span></div>}
                        {typeof flowSummary.renders === 'number' && <div className="flex justify-between text-xs"><span className="text-zinc-700">Renders</span><span className="text-zinc-600">{flowSummary.renders}</span></div>}
                        {typeof flowSummary.injects === 'number' && <div className="flex justify-between text-xs"><span className="text-zinc-700">Injects</span><span className="text-zinc-600">{flowSummary.injects}</span></div>}
                      </>
                    )}
                  </div>
                )}
              </>
            )}
          </div>
          <p className="text-[10px] text-zinc-500 px-3 py-2 border-t border-zinc-200">Click a node to inspect · Scroll to zoom</p>
        </div>

        {/* Graph canvas */}
        <div ref={graphPanelRef} className="relative flex min-h-0 flex-1 overflow-hidden bg-zinc-100">
          <div ref={containerRef} className="h-full min-h-0 w-full min-w-0" />

          {loading && (
            <div className="absolute inset-0 flex items-center justify-center bg-white/85 backdrop-blur-[2px] z-20">
              <div className="flex flex-col items-center gap-2">
                <Loader2 className="w-8 h-8 animate-spin text-blue-600" />
                <span className="text-sm text-zinc-600">Loading graph…</span>
              </div>
            </div>
          )}

          {loadError && (
            <div className="absolute inset-0 flex items-center justify-center bg-white/85 backdrop-blur-[2px] z-20">
              <div className="flex flex-col items-center gap-2 text-center max-w-sm">
                <AlertCircle className="w-8 h-8 text-red-600" />
                <p className="text-sm text-red-700">{loadError}</p>
                <Button size="sm" variant="outline" onClick={loadGraph}>Retry</Button>
              </div>
            </div>
          )}

          {/* Node detail panel — Task 5/6 */}
          {selectedNode && detailPos && (
            <div
              className="absolute z-30 w-64 bg-white border border-zinc-200 rounded-lg shadow-xl pointer-events-auto"
              style={{
                left: Math.max(8, Math.min(detailPos.x, (graphPanelRef.current?.clientWidth || 800) - 268)),
                top: Math.max(8, Math.min(detailPos.y, (graphPanelRef.current?.clientHeight || 600) - 300)),
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

          {/* Zoom + minimap */}
          <div className="pointer-events-auto absolute bottom-3 right-3 z-40 flex flex-col items-end gap-2">
            <div className="flex flex-row gap-1">
              <Button size="icon" variant="outline" className="h-8 w-8 border-zinc-300 bg-white/95 shadow-sm" onClick={zoomIn}><Plus className="h-4 w-4" /></Button>
              <Button size="icon" variant="outline" className="h-8 w-8 border-zinc-300 bg-white/95 shadow-sm" onClick={zoomOut}><Minus className="h-4 w-4" /></Button>
              <Button size="icon" variant="outline" className="h-8 w-8 border-zinc-300 bg-white/95 shadow-sm" onClick={zoomFit}><Maximize className="h-4 w-4" /></Button>
            </div>
            {/* Minimap container — Task 7 */}
            <div className="overflow-hidden rounded-lg border border-zinc-200 bg-white shadow-md w-[160px]">
              <div className="px-2 py-1 text-[9px] text-zinc-400 uppercase tracking-wider border-b border-zinc-100 bg-zinc-50 select-none">
                Overview · drag to navigate
              </div>
              <canvas
                ref={minimapCanvasRef}
                className="block cursor-crosshair"
                aria-label="Graph overview — click or drag to navigate"
                style={{ width: 160, height: 92 }}
              />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
```

Note: `focusNode` is referenced in the JSX — add a stub above `clearSelection`:
```tsx
const focusNode = useCallback((_id: string) => { /* Task 5 */ }, []);
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
cd ui && npx tsc --noEmit 2>&1 | grep -c "error TS" || echo "0 errors"
```

Expected: 0 TypeScript errors (or only errors about the stub `buildGraph` not using `_data`).

- [ ] **Step 3: Commit**

```bash
git add ui/src/pages/GraphPage.tsx
git commit -m "refactor: replace force-graph imports with sigma skeleton in GraphPage"
```

---

### Task 4: Implement `buildGraph` — graphology + sigma instantiation

**Files:**
- Modify: `ui/src/pages/GraphPage.tsx`

Replace the stub `buildGraph` callback with the full implementation. This gives us a working (static) render.

- [ ] **Step 1: Replace `buildGraph` stub with full implementation**

Replace the entire `buildGraph` useCallback body (the `// Task 4` stub):

```tsx
const buildGraph = useCallback((data: GraphData) => {
  if (!containerRef.current) return;

  // Tear down existing instance
  layoutTimerRef.current && clearInterval(layoutTimerRef.current);
  layoutRef.current?.kill();
  sigmaRef.current?.kill();
  sigmaRef.current = null;
  graphRef.current = null;

  // ── 1. Filter nodes ──────────────────────────────────────────────────────
  let nodes = [...(data.nodes || [])];
  if (labelFilter !== 'All') nodes = nodes.filter(n => n.label === labelFilter);
  if (searchQuery) {
    const q = searchQuery.toLowerCase();
    nodes = nodes.filter(n =>
      (n.name || '').toLowerCase().includes(q) ||
      ((n as any).qualified_name || '').toLowerCase().includes(q),
    );
  }
  const nodeIds = new Set(nodes.map(n => String(n.id)));

  // ── 2. Filter edges ──────────────────────────────────────────────────────
  let links = [...((data as any).links || data.edges || [])];
  links = links.filter(l => {
    const sid = String(typeof l.source === 'object' ? l.source.id : l.source);
    const tid = String(typeof l.target === 'object' ? l.target.id : l.target);
    if (!nodeIds.has(sid) || !nodeIds.has(tid)) return false;
    if (edgeFilter !== 'All' && l.type !== edgeFilter) return false;
    return true;
  });

  // ── 3. Synthetic CONTAINS edges ──────────────────────────────────────────
  let hasSynthetic = false;
  if (!links.some(l => l.type === 'CONTAINS')) {
    const folders = new Map<string, string>();
    for (const n of nodes) {
      if (n.label === 'Folder') folders.set(n.file_path || n.name || '', String(n.id));
    }
    for (const n of nodes) {
      if (n.file_path && n.label !== 'Folder') {
        const dir = n.file_path.split('/').slice(0, -1).join('/');
        const fid = folders.get(dir);
        if (fid !== undefined && fid !== String(n.id)) {
          links.push({ source: fid, target: String(n.id), type: 'CONTAINS' });
          hasSynthetic = true;
        }
      }
    }
  }
  setSyntheticEdges(hasSynthetic);

  // ── 4. Degree map for label priority ────────────────────────────────────
  const degree = new Map<string, number>();
  for (const l of links) {
    const sid = String(typeof l.source === 'object' ? l.source.id : l.source);
    const tid = String(typeof l.target === 'object' ? l.target.id : l.target);
    degree.set(sid, (degree.get(sid) || 0) + 1);
    degree.set(tid, (degree.get(tid) || 0) + 1);
  }
  const topLabelIds = new Set(
    [...degree.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 15)
      .map(([id]) => id),
  );

  // ── 5. Build graphology graph ────────────────────────────────────────────
  const graph = new MultiDirectedGraph();

  for (const n of nodes) {
    graph.addNode(String(n.id), {
      label: n.name || String(n.id),
      x: Math.random(),
      y: Math.random(),
      size: NODE_SIZES[n.label] || 3,
      color: NODE_COLORS[n.label] || '#888',
      nodeType: n.label,
      filePath: n.file_path || '',
      rawId: n.id,
    });
  }

  for (const l of links) {
    const sid = String(typeof l.source === 'object' ? l.source.id : l.source);
    const tid = String(typeof l.target === 'object' ? l.target.id : l.target);
    if (graph.hasNode(sid) && graph.hasNode(tid)) {
      try {
        graph.addEdge(sid, tid, {
          edgeType: l.type || '',
          color: EDGE_COLORS[l.type] || '#71717a',
          size: 1,
        });
      } catch { /* skip duplicate edges in multi-graph */ }
    }
  }

  setFilteredNodeCount(graph.order);
  setFilteredEdgeCount(graph.size);

  // ── 6. Create sigma instance ─────────────────────────────────────────────
  highlightedNodes.current = new Set();
  highlightedEdges.current = new Set();
  selectedNodeIdRef.current = null;
  setSelectedNode(null);
  setDetailPos(null);
  setConnectedCallers([]);
  setConnectedCallees([]);

  const sigma = new Sigma(graph, containerRef.current, {
    renderEdgeLabels: false,
    labelFont: 'Geist Variable, sans-serif',
    labelSize: 11,
    labelWeight: '500',
    labelColor: { color: '#27272a' },
    defaultNodeColor: '#888',
    defaultEdgeColor: '#71717a',
    nodeReducer: (node, data) => {
      const hl = highlightedNodes.current;
      const ratio = sigmaRef.current?.getCamera().getState().ratio ?? 1;
      const isHl = hl.size === 0 || hl.has(node);
      // Show label: always when zoomed in (ratio < 0.5), else only top-15 by degree
      const showLabel = isHl && (ratio < 0.5 || topLabelIds.has(node));
      return {
        ...data,
        color: isHl ? (data.color as string) : '#d4d4d8',
        label: showLabel ? (data.label as string) : '',
      };
    },
    edgeReducer: (edge, data) => {
      const hl = highlightedEdges.current;
      if (hl.size > 0 && !hl.has(edge)) return { ...data, color: '#e4e4e7', size: 0.3 };
      if (hl.has(edge)) return { ...data, size: 2 };
      return data;
    },
  });

  sigmaRef.current = sigma;
  graphRef.current = graph;

  // Wire camera → minimap + detail panel
  sigma.getCamera().on('updated', () => {
    drawMinimap();
    if (selectedNodeIdRef.current) updateDetailPanelPos();
  });

  // ── 7. FA2 layout worker ─────────────────────────────────────────────────
  const nodeCount = graph.order;
  const layout = new FA2Layout(graph, {
    settings: {
      barnesHutOptimize: nodeCount > 500,
      gravity: 1,
      scalingRatio: 2,
      slowDown: 1 + Math.log(Math.max(1, nodeCount)),
    },
  });
  layout.start();
  layoutRef.current = layout;
  setSimulating(true);

  const maxMs = nodeCount > 500 ? 10000 : 4000;
  const started = Date.now();
  layoutTimerRef.current = setInterval(() => {
    sigma.refresh();
    drawMinimap();
    if (!layout.running || Date.now() - started > maxMs) {
      layout.stop();
      clearInterval(layoutTimerRef.current!);
      setSimulating(false);
      sigma.refresh();
      drawMinimap();
    }
  }, 50);

  // Initial draw
  drawMinimap();
}, [labelFilter, searchQuery, edgeFilter, clearSelection, drawMinimap, updateDetailPanelPos]);
// NOTE: selectNode is added to this dep array in Task 5 when click handlers are wired.
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
cd ui && npx tsc --noEmit 2>&1 | grep "error TS" | head -10
```

Expected: 0 errors.

- [ ] **Step 3: Verify dev server runs and renders graph**

```bash
cd ui && npm run dev
```

Open `http://localhost:5173/graph?project=<any-indexed-project>`. You should see a sigma canvas with colored nodes on a plain white background. The layout will animate for a few seconds. Labels should appear when zoomed in.

- [ ] **Step 4: Commit**

```bash
git add ui/src/pages/GraphPage.tsx
git commit -m "feat: implement sigma.js WebGL graph renderer with FA2 layout worker"
```

---

### Task 5: Node selection + highlighting + `focusNode`

**Files:**
- Modify: `ui/src/pages/GraphPage.tsx`

- [ ] **Step 1: Replace `focusNode` stub and add `selectNode`**

Replace the `focusNode` stub and add `selectNode` (place these after `clearSelection`):

```tsx
const selectNode = useCallback((nodeId: string) => {
  const sigma = sigmaRef.current;
  const graph = graphRef.current;
  if (!sigma || !graph || !graph.hasNode(nodeId)) return;

  const hNodes = new Set<string>([nodeId]);
  const hEdges = new Set<string>();
  const callers: NeighborNode[] = [];
  const callees: NeighborNode[] = [];

  graph.forEachEdge((edge, _attrs, source, target) => {
    if (source === nodeId) {
      hNodes.add(target);
      hEdges.add(edge);
      const attrs = graph.getNodeAttributes(target);
      callees.push({ id: target, name: attrs.label as string, label: attrs.nodeType as string });
    }
    if (target === nodeId) {
      hNodes.add(source);
      hEdges.add(edge);
      const attrs = graph.getNodeAttributes(source);
      callers.push({ id: source, name: attrs.label as string, label: attrs.nodeType as string });
    }
  });

  highlightedNodes.current = hNodes;
  highlightedEdges.current = hEdges;
  selectedNodeIdRef.current = nodeId;

  const nodeAttrs = graph.getNodeAttributes(nodeId);
  // Build a GraphNode-shaped object for the detail panel
  const syntheticGraphNode: GraphNode = {
    id: nodeAttrs.rawId as number,
    name: nodeAttrs.label as string,
    label: nodeAttrs.nodeType as string,
    file_path: nodeAttrs.filePath as string || undefined,
    x: 0, y: 0, size: 0, color: '',
  };
  setSelectedNode(syntheticGraphNode);
  setConnectedCallers(callers);
  setConnectedCallees(callees);

  sigma.refresh();
  updateDetailPanelPos();

  // Center camera on node
  const gAttrs = graph.getNodeAttributes(nodeId);
  if (gAttrs.x != null) {
    const norm = (sigma as any).normalizationFunction?.applyTo?.({ x: gAttrs.x as number, y: gAttrs.y as number })
      ?? { x: 0.5, y: 0.5 };
    sigma.getCamera().animate({ x: norm.x, y: norm.y }, { duration: 500 });
  }
}, [updateDetailPanelPos]);

const focusNode = useCallback((id: string) => {
  selectNode(id);
}, [selectNode]);
```

- [ ] **Step 2: Wire sigma click events inside `buildGraph`**

Inside `buildGraph`, after creating the sigma instance, replace the placeholder `// Wire camera → minimap + detail panel` section to also add:

```tsx
sigma.on('clickNode', ({ node }) => selectNode(node));
sigma.on('clickStage', () => clearSelection());
```

**Important:** Add `selectNode` to `buildGraph`'s `useCallback` dependency array. The final array should be:
`[labelFilter, searchQuery, edgeFilter, selectNode, clearSelection, drawMinimap, updateDetailPanelPos]`

- [ ] **Step 3: Verify in browser**

Run dev server. Click a node — it should highlight in color while neighbors dim to gray. Clicking background clears highlight. The detail panel div renders but floats at (0,0) until Task 6.

- [ ] **Step 4: Commit**

```bash
git add ui/src/pages/GraphPage.tsx
git commit -m "feat: implement node selection and neighbor highlighting with sigma reducers"
```

---

### Task 6: Detail panel position tracking

**Files:**
- Modify: `ui/src/pages/GraphPage.tsx`

Replace the `updateDetailPanelPos` stub.

- [ ] **Step 1: Implement `updateDetailPanelPos`**

Replace the `// Task 6` stub:

```tsx
const updateDetailPanelPos = useCallback(() => {
  const nodeId = selectedNodeIdRef.current;
  const sigma = sigmaRef.current;
  const graph = graphRef.current;
  const panel = graphPanelRef.current;
  if (!nodeId || !sigma || !graph || !panel || !graph.hasNode(nodeId)) return;

  const gAttrs = graph.getNodeAttributes(nodeId);
  if (gAttrs.x == null) return;

  // Convert graph coords → viewport (CSS pixel) coords
  const { x: vpX, y: vpY } = sigma.graphToViewport({
    x: gAttrs.x as number,
    y: gAttrs.y as number,
  });

  const nodeSize = (gAttrs.size as number) || 3;
  const panelW = 264;
  const panelH = 300;
  const MINIMAP_ZONE_W = 172; // avoid bottom-right 172×150 zone
  const MINIMAP_ZONE_H = 150;
  const pW = panel.clientWidth;
  const pH = panel.clientHeight;

  let x = vpX + nodeSize + 12;
  let y = vpY - 10;

  // Keep on screen
  x = Math.max(8, Math.min(x, pW - panelW - 8));
  y = Math.max(8, Math.min(y, pH - panelH - 8));

  // Push left if overlapping minimap zone (bottom-right corner)
  const inMinimapX = x + panelW > pW - MINIMAP_ZONE_W;
  const inMinimapY = y + panelH > pH - MINIMAP_ZONE_H;
  if (inMinimapX && inMinimapY) {
    x = Math.max(8, pW - MINIMAP_ZONE_W - panelW - 12);
  }

  setDetailPos({ x, y });
}, []);
```

- [ ] **Step 2: Verify in browser**

Click a node. The detail panel should appear to the right of the node and track it as you pan (drag the canvas background) or zoom.

- [ ] **Step 3: Commit**

```bash
git add ui/src/pages/GraphPage.tsx
git commit -m "feat: node detail panel tracks selected node position via sigma camera events"
```

---

### Task 7: Interactive minimap

**Files:**
- Modify: `ui/src/pages/GraphPage.tsx`

Replace `drawMinimap` stub and add minimap mouse handlers.

- [ ] **Step 1: Implement `drawMinimap`**

Replace the `// Task 7` stub:

```tsx
const drawMinimap = useCallback(() => {
  const canvas = minimapCanvasRef.current;
  const sigma = sigmaRef.current;
  const graph = graphRef.current;
  if (!canvas || !sigma || !graph || graph.order === 0) return;

  const ctx = canvas.getContext('2d');
  if (!ctx) return;

  const CSS_W = 160;
  const CSS_H = 92;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  if (canvas.width !== CSS_W * dpr) {
    canvas.width = CSS_W * dpr;
    canvas.height = CSS_H * dpr;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

  ctx.fillStyle = '#f4f4f5';
  ctx.fillRect(0, 0, CSS_W, CSS_H);

  // Compute graph bounding box
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  graph.forEachNode((_, attrs) => {
    const x = attrs.x as number, y = attrs.y as number;
    if (x != null) { minX = Math.min(minX, x); maxX = Math.max(maxX, x); }
    if (y != null) { minY = Math.min(minY, y); maxY = Math.max(maxY, y); }
  });
  if (!isFinite(minX)) return;

  const pad = 6;
  const rangeX = (maxX - minX) || 1;
  const rangeY = (maxY - minY) || 1;
  const scale = Math.min((CSS_W - pad * 2) / rangeX, (CSS_H - pad * 2) / rangeY);
  const ox = (CSS_W - rangeX * scale) / 2;
  const oy = (CSS_H - rangeY * scale) / 2;

  const toMx = (x: number) => (x - minX) * scale + ox;
  const toMy = (y: number) => (y - minY) * scale + oy;

  // Compute normalized range for drag conversion
  let normMinX = Infinity, normMaxX = -Infinity, normMinY = Infinity, normMaxY = -Infinity;
  const normFn = (sigma as any).normalizationFunction;
  if (normFn?.applyTo) {
    const tl = normFn.applyTo({ x: minX, y: minY });
    const br = normFn.applyTo({ x: maxX, y: maxY });
    normMinX = tl.x; normMaxX = br.x; normMinY = tl.y; normMaxY = br.y;
  } else {
    // Fallback: assume sigma normalizes to [0,1]
    normMinX = 0; normMaxX = 1; normMinY = 0; normMaxY = 1;
  }

  minimapTransform.current = {
    minX, maxX, minY, maxY, scale, ox, oy,
    normRangeX: Math.abs(normMaxX - normMinX) || 1,
    normRangeY: Math.abs(normMaxY - normMinY) || 1,
  };

  // Draw edges
  ctx.globalAlpha = 0.35;
  ctx.strokeStyle = '#a1a1aa';
  ctx.lineWidth = 0.6;
  graph.forEachEdge((_, __, source, target) => {
    const s = graph.getNodeAttributes(source);
    const t = graph.getNodeAttributes(target);
    if (s.x == null || t.x == null) return;
    ctx.beginPath();
    ctx.moveTo(toMx(s.x as number), toMy(s.y as number));
    ctx.lineTo(toMx(t.x as number), toMy(t.y as number));
    ctx.stroke();
  });

  // Draw nodes
  ctx.globalAlpha = 1;
  graph.forEachNode((_, attrs) => {
    if (attrs.x == null) return;
    ctx.fillStyle = (attrs.color as string) || '#888';
    ctx.beginPath();
    ctx.arc(toMx(attrs.x as number), toMy(attrs.y as number), 1.5, 0, Math.PI * 2);
    ctx.fill();
  });

  // Draw viewport rect
  const container = containerRef.current;
  if (!container) return;
  const tl = sigma.viewportToGraph({ x: 0, y: 0 });
  const br = sigma.viewportToGraph({ x: container.clientWidth, y: container.clientHeight });
  const rx = toMx(tl.x), ry = toMy(tl.y);
  const rw = toMx(br.x) - rx, rh = toMy(br.y) - ry;

  ctx.strokeStyle = '#15803d';
  ctx.lineWidth = 1.2;
  ctx.setLineDash([3, 2]);
  ctx.globalAlpha = 0.9;
  ctx.strokeRect(
    Math.max(0, rx) + 0.5,
    Math.max(0, ry) + 0.5,
    Math.min(CSS_W - Math.max(0, rx), rw) - 1,
    Math.min(CSS_H - Math.max(0, ry), rh) - 1,
  );
  ctx.setLineDash([]);
  ctx.globalAlpha = 1;
}, []);
```

- [ ] **Step 2: Add minimap mouse event handlers**

Add these three handlers below `drawMinimap`:

```tsx
const handleMinimapMouseDown = useCallback((e: React.MouseEvent<HTMLCanvasElement>) => {
  e.preventDefault();
  const sigma = sigmaRef.current;
  const t = minimapTransform.current;
  if (!sigma || !t) return;

  const rect = (e.target as HTMLCanvasElement).getBoundingClientRect();
  const mx = e.clientX - rect.left;
  const my = e.clientY - rect.top;

  // Convert minimap pixel → graph space
  const gx = (mx - t.ox) / t.scale + t.minX;
  const gy = (my - t.oy) / t.scale + t.minY;

  // Convert graph → normalized camera space via sigma's normalizationFunction
  const normFn = (sigma as any).normalizationFunction;
  const norm = normFn?.applyTo?.({ x: gx, y: gy }) ?? { x: 0.5, y: 0.5 };

  sigma.getCamera().animate({ x: norm.x, y: norm.y }, { duration: 250 });
  minimapDragging.current = true;
}, []);

const handleMinimapMouseMove = useCallback((e: React.MouseEvent<HTMLCanvasElement>) => {
  if (!minimapDragging.current) return;
  e.preventDefault();
  const sigma = sigmaRef.current;
  const t = minimapTransform.current;
  if (!sigma || !t) return;

  // dx in minimap pixels → dx in normalized camera units
  // normRangeX / scale = normalized units per minimap pixel (within the graph extent)
  const dxNorm = (e.movementX / t.scale / (t.maxX - t.minX)) * t.normRangeX;
  const dyNorm = (e.movementY / t.scale / (t.maxY - t.minY)) * t.normRangeY;

  const cur = sigma.getCamera().getState();
  sigma.getCamera().setState({ x: cur.x + dxNorm, y: cur.y + dyNorm });
}, []);

const handleMinimapMouseUp = useCallback(() => {
  minimapDragging.current = false;
}, []);
```

- [ ] **Step 3: Wire handlers onto the minimap canvas**

In the JSX, update the `<canvas>` element inside the minimap container:

```tsx
<canvas
  ref={minimapCanvasRef}
  className="block cursor-crosshair"
  aria-label="Graph overview — click or drag to navigate"
  style={{ width: 160, height: 92 }}
  onMouseDown={handleMinimapMouseDown}
  onMouseMove={handleMinimapMouseMove}
  onMouseUp={handleMinimapMouseUp}
  onMouseLeave={handleMinimapMouseUp}
/>
```

- [ ] **Step 4: Verify in browser**

Minimap should show nodes/edges with a dashed viewport rect. Clicking the minimap should animate the main camera to that region. Dragging should pan the camera continuously.

- [ ] **Step 5: Commit**

```bash
git add ui/src/pages/GraphPage.tsx
git commit -m "feat: interactive minimap with click-to-navigate and drag-to-pan"
```

---

### Task 8: Verify filtering

**Files:**
- Modify: `ui/src/pages/GraphPage.tsx`

Filtering (search input, label select, edge type select) rebuilds the graphology graph from `rawData`. The `buildGraph` function already handles this correctly via the `labelFilter`, `searchQuery`, and `edgeFilter` dependencies — and the `useEffect` that calls `buildGraph(rawData)` when those values change is already wired in Task 3. This task just verifies it works.

- [ ] **Step 1: Manually test all three filters**

In the dev server:
1. Type a node name in the search input — graph should re-render with only matching nodes.
2. Select a node type from the dropdown (e.g. "Function") — graph rebuilds showing only Function nodes.
3. Select an edge type (e.g. "CALLS") — graph rebuilds with only CALLS edges.
4. Select "All" in each — returns to full graph.

Expected: all three filters rebuild the sigma instance and re-run FA2 layout.

- [ ] **Step 2: Verify TypeScript compiles cleanly**

```bash
cd ui && npx tsc --noEmit 2>&1 | grep "error TS" | head -5
```

Expected: 0 errors.

- [ ] **Step 3: Commit (if any adjustments were needed)**

```bash
git add ui/src/pages/GraphPage.tsx
git commit -m "fix: verify filtering rebuilds sigma graph correctly"
```

---

### Task 9: Production build + final verification

**Files:**
- No code changes — verification only.

- [ ] **Step 1: Run production build**

```bash
cd ui && npm run build 2>&1 | tail -15
```

Expected: build completes, `dist/` updated, no TypeScript or Vite errors.

- [ ] **Step 2: Smoke-test the preview build**

```bash
cd ui && npm run preview
```

Open `http://localhost:4173/graph?project=<your-project>`. Verify:
- No page scroll (graph fills viewport below header)
- Nodes render with correct colors
- FA2 layout animates and stops
- Click a node → detail panel appears beside it, tracks on pan/zoom
- Detail panel never covers the minimap
- Minimap: click to jump, drag to pan
- Zoom in/out/fit buttons work
- Search, label filter, edge filter all rebuild the graph
- Flow tab (Routes / Components) still works

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: graph page enhancement complete — sigma WebGL, interactive minimap, layout fixes"
```

---

## Appendix: sigma.js coordinate system notes

- **Camera ratio:** `ratio = 1` is default zoom. `ratio < 1` = zoomed in (nodes appear larger). `ratio > 1` = zoomed out.
- **Camera x, y:** Position of the camera center in sigma's **normalized graph space** (approximately [0, 1] range after sigma normalizes all node positions to fit a unit square).
- **`sigma.graphToViewport({ x, y })`**: Converts graphology node positions (FA2 output coordinates, arbitrary range) → viewport pixel coordinates.
- **`sigma.viewportToGraph({ x, y })`**: Viewport pixels → graphology coordinates.
- **`sigma.normalizationFunction.applyTo({ x, y })`**: Graphology coordinates → normalized camera space. Used in minimap click handler to animate the camera to a specific graph-space location.
- **`nodeReducer` / `edgeReducer`**: Called on every sigma render frame. Use refs (not state) for highlight sets so reducer always reads fresh values without triggering re-renders.
