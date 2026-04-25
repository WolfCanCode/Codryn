import { useCallback, useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { AlertCircle, Box, GitBranch, Loader2, Search, Server, Workflow } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { CanvasGraphView } from '@/components/graph/canvas/CanvasGraphView';
import { FlowSankeyView } from '@/components/flow/FlowSankeyView';
import { OnboardingExtract } from '@/components/flow/OnboardingExtract';
import { PipelineDagView } from '@/components/PipelineDagView';
import {
  fetchBackendFlowGraph,
  fetchBackendFlowList,
  fetchFrontendFlowGraph,
  fetchFrontendFlowList,
  fetchInfrastructure,
  fetchLayout,
  fetchPipelineDag,
  fetchPipelines,
} from '@/lib/rpc';
import type { GraphData, GraphEdge, GraphNode, InfraResource, PipelineDag } from '@/lib/types';
import type {
  BackendFlowListItem,
  BackendFlowListResponse,
  BackendFlowResponse,
  FlowNode,
  FrontendFlowListItem,
  FrontendFlowListResponse,
  FrontendFlowResponse,
  SankeyGraph,
} from '@/components/flow/flowTypes';
import type { FlowMode } from '@/components/flow/flowLane';
import { toSankeyGraph } from '@/components/flow/sankeyModel';

function buildArchitectureGraph(raw: GraphData, depth: 1 | 2 | 3): GraphData {
  const nodes = raw.nodes ?? [];
  const edges = raw.edges ?? [];

  const hueFromString = (s: string) => {
    let h = 0;
    for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
    return h % 360;
  };

  const groupKey = (n: GraphNode) => {
    const fp = (n.file_path || '').trim();
    if (!fp) return n.label;
    const parts = fp.split('/').filter(Boolean);
    const d = Math.min(depth, parts.length);
    return parts.slice(0, d).join('/');
  };

  const groupIdByKey = new Map<string, number>();
  const groupNodes: GraphNode[] = [];
  let nextId = 1;

  const ensureGroup = (k: string) => {
    const existing = groupIdByKey.get(k);
    if (existing) return existing;
    const id = nextId++;
    groupIdByKey.set(k, id);
    groupNodes.push({
      id,
      x: 0,
      y: 0,
      label: 'Module',
      name: k,
      size: 10,
      color: `hsl(${hueFromString(k)}, 55%, 45%)`,
      file_path: k,
    });
    return id;
  };

  const groupForNodeId = new Map<number, number>();
  for (const n of nodes) {
    const k = groupKey(n);
    const gid = ensureGroup(k);
    groupForNodeId.set(n.id, gid);
  }

  const agg = new Map<string, number>();
  for (const e of edges) {
    const s = groupForNodeId.get(e.source);
    const t = groupForNodeId.get(e.target);
    if (!s || !t || s === t) continue;
    const k = s < t ? `${s}\0${t}` : `${t}\0${s}`;
    agg.set(k, (agg.get(k) ?? 0) + 1);
  }

  const outEdges: GraphEdge[] = [];
  for (const [k] of agg) {
    const [a, b] = k.split('\0').map((x) => Number(x));
    outEdges.push({ source: a, target: b, type: 'CALLS' });
  }

  return { nodes: groupNodes, edges: outEdges, total_nodes: groupNodes.length };
}

export default function GraphPage() {
  const [searchParams] = useSearchParams();
  const project = searchParams.get('project') || '';

  const [activeTab, setActiveTab] = useState<'graph' | 'architecture' | 'pipelines' | 'infrastructure'>('graph');

  // Architecture graph data
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [rawData, setRawData] = useState<GraphData | null>(null);
  const [archDepth, setArchDepth] = useState<1 | 2 | 3>(1);
  const [selectedArchNode, setSelectedArchNode] = useState<GraphNode | null>(null);

  // Pipelines
  const [pipelineList, setPipelineList] = useState<PipelineDag[]>([]);
  const [pipelineLoading, setPipelineLoading] = useState(false);
  const [selectedPipeline, setSelectedPipeline] = useState<PipelineDag | null>(null);
  const [selectedPipelineDag, setSelectedPipelineDag] = useState<PipelineDag | null>(null);

  // Infra
  const [infrastructure, setInfrastructure] = useState<InfraResource[]>([]);
  const [infraLoading, setInfraLoading] = useState(false);
  const [includeLinked, setIncludeLinked] = useState(true);

  // Flow Sankey Explorer
  const [flowMode, setFlowMode] = useState<FlowMode>('route');
  const [flowAnchorQuery, setFlowAnchorQuery] = useState('');
  const [backendAnchors, setBackendAnchors] = useState<BackendFlowListItem[]>([]);
  const [frontendAnchors, setFrontendAnchors] = useState<FrontendFlowListItem[]>([]);
  const [anchorLoading, setAnchorLoading] = useState(false);

  const [selectedAnchorKey, setSelectedAnchorKey] = useState('');
  const [selectedAnchorTitle, setSelectedAnchorTitle] = useState('');
  const [sankeyGraph, setSankeyGraph] = useState<SankeyGraph | null>(null);
  const [selectedSankeyNodeId, setSelectedSankeyNodeId] = useState<string | undefined>(undefined);
  const [flowNodesBySankeyId, setFlowNodesBySankeyId] = useState<Map<string, FlowNode>>(() => new Map());
  const [flowConfidence, setFlowConfidence] = useState<number | undefined>(undefined);
  const [sankeyLoading, setSankeyLoading] = useState(false);

  // Load architecture raw layout
  const loadLayoutData = useCallback(async () => {
    if (!project) return;
    setLoading(true);
    setLoadError(null);
    try {
      const res = await fetchLayout(project, 8000);
      setRawData(res);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : 'Failed to load graph');
    } finally {
      setLoading(false);
    }
  }, [project]);

  useEffect(() => {
    if (activeTab !== 'architecture') return;
    if (rawData) return;
    const t = window.setTimeout(() => void loadLayoutData(), 0);
    return () => window.clearTimeout(t);
  }, [activeTab, loadLayoutData, rawData]);

  const architectureData = useMemo(() => {
    if (activeTab !== 'architecture') return null;
    if (!rawData) return null;
    return buildArchitectureGraph(rawData, archDepth);
  }, [activeTab, archDepth, rawData]);

  // Load anchors for Sankey tab
  const loadFlowAnchors = useCallback(async () => {
    if (!project) return;
    setAnchorLoading(true);
    try {
      if (flowMode === 'route') {
        const res = (await fetchBackendFlowList(project)) as BackendFlowListResponse;
        setBackendAnchors(res.routes ?? []);
      } else {
        const res = (await fetchFrontendFlowList(project)) as FrontendFlowListResponse;
        setFrontendAnchors(res.components ?? []);
      }
    } finally {
      setAnchorLoading(false);
    }
  }, [flowMode, project]);

  useEffect(() => {
    if (activeTab !== 'graph') return;
    const t = window.setTimeout(() => void loadFlowAnchors(), 0);
    return () => window.clearTimeout(t);
  }, [activeTab, loadFlowAnchors]);

  const graphTabAnchors = useMemo(() => {
    const MAX = 200;
    const q = flowAnchorQuery.trim().toLowerCase();
    if (flowMode === 'route') {
      const sorted = [...backendAnchors].sort((a, b) => (a.path || '').localeCompare(b.path || ''));
      const filtered = !q
        ? sorted
        : sorted.filter((r) => `${r.method || ''} ${r.path || ''} ${r.handler || ''}`.toLowerCase().includes(q));
      return { total: filtered.length, items: filtered.slice(0, MAX), capped: filtered.length > MAX };
    }
    const sorted = [...frontendAnchors].sort((a, b) => (a.name || '').localeCompare(b.name || ''));
    const filtered = !q
      ? sorted
      : sorted.filter((c) => `${c.name || ''}\n${c.file_path || ''}`.toLowerCase().includes(q));
    return { total: filtered.length, items: filtered.slice(0, MAX), capped: filtered.length > MAX };
  }, [backendAnchors, flowAnchorQuery, flowMode, frontendAnchors]);

  const selectFlowAnchor = useCallback(
    async (key: string, title: string, fetcher: () => Promise<BackendFlowResponse | FrontendFlowResponse>) => {
      if (!project) return;
      setSelectedAnchorKey(key);
      setSelectedAnchorTitle(title);
      setSelectedSankeyNodeId(undefined);
      setSankeyLoading(true);
      try {
        const flow = await fetcher();
        if (flow?.error) return;
        const g = toSankeyGraph(flowMode, flow);
        setSankeyGraph(g);
        setFlowConfidence(flow?.summary?.confidence);

        const byFlowId = new Map<string, FlowNode>();
        for (const n of flow.nodes ?? []) byFlowId.set(String(n.id), n);
        const m = new Map<string, FlowNode>();
        for (const n of g.nodes) {
          const raw = (n.rawNodeId ?? '').trim();
          if (!raw) continue;
          const original = byFlowId.get(raw);
          if (original) m.set(n.id, original);
        }
        setFlowNodesBySankeyId(m);
      } finally {
        setSankeyLoading(false);
      }
    },
    [flowMode, project],
  );

  const onSelectBackendAnchor = useCallback(
    (r: BackendFlowListItem) => {
      const method = (r.method || '').toUpperCase();
      const path = r.path || '';
      const title = `${method} ${path}`.trim();
      const key = `route:${method}:${path}`;
      void selectFlowAnchor(key, title, () => fetchBackendFlowGraph(project, path, method));
    },
    [project, selectFlowAnchor],
  );

  const onSelectFrontendAnchor = useCallback(
    (c: FrontendFlowListItem, listKey: string) => {
      const name = c.name || '';
      const title = name.trim() || '(unnamed component)';
      void selectFlowAnchor(listKey, title, () => fetchFrontendFlowGraph(project, name));
    },
    [project, selectFlowAnchor],
  );

  // Pipelines
  const loadPipelines = useCallback(async () => {
    if (!project) return;
    setPipelineLoading(true);
    try {
      const res = await fetchPipelines(project, includeLinked);
      setPipelineList(res.pipelines ?? []);
    } finally {
      setPipelineLoading(false);
    }
  }, [includeLinked, project]);

  useEffect(() => {
    if (activeTab !== 'pipelines') return;
    const t = window.setTimeout(() => void loadPipelines(), 0);
    return () => window.clearTimeout(t);
  }, [activeTab, loadPipelines]);

  const selectPipeline = useCallback(
    async (pipeline: PipelineDag) => {
      if (!project) return;
      setSelectedPipeline(pipeline);
      setSelectedPipelineDag(null);
      try {
        const dag = await fetchPipelineDag(project, pipeline.pipeline.name, pipeline.source_project, includeLinked);
        setSelectedPipelineDag(dag);
      } catch {
        setSelectedPipelineDag(pipeline);
      }
    },
    [includeLinked, project],
  );

  // Infra
  const loadInfrastructure = useCallback(async () => {
    if (!project) return;
    setInfraLoading(true);
    try {
      const res = await fetchInfrastructure(project, undefined, includeLinked);
      setInfrastructure(res.resources ?? []);
    } finally {
      setInfraLoading(false);
    }
  }, [includeLinked, project]);

  useEffect(() => {
    if (activeTab !== 'infrastructure') return;
    const t = window.setTimeout(() => void loadInfrastructure(), 0);
    return () => window.clearTimeout(t);
  }, [activeTab, loadInfrastructure]);

  return (
    <main className="mx-auto flex w-full max-w-[1440px] min-h-0 flex-1 flex-col overflow-hidden bg-zinc-100">
      <div className="flex min-h-0 flex-1 overflow-hidden">
        {/* Left panel */}
        <aside className="w-[360px] shrink-0 overflow-hidden border-r border-zinc-200 bg-white">
          <div className="grid grid-cols-4 border-b border-zinc-200">
            <button
              type="button"
              onClick={() => setActiveTab('graph')}
              className={cn(
                'flex flex-col items-center justify-center gap-1.5 border-b-2 py-3.5 text-[11px] font-medium transition-colors',
                activeTab === 'graph' ? 'border-zinc-900 bg-white text-zinc-900' : 'border-transparent text-zinc-500 hover:bg-white/70 hover:text-zinc-800',
              )}
            >
              <GitBranch className="h-3.5 w-3.5 shrink-0 opacity-90" aria-hidden />
              <span className="leading-none">Graph</span>
            </button>
            <button
              type="button"
              onClick={() => setActiveTab('architecture')}
              className={cn(
                'flex flex-col items-center justify-center gap-1.5 border-b-2 py-3.5 text-[11px] font-medium transition-colors',
                activeTab === 'architecture' ? 'border-zinc-900 bg-white text-zinc-900' : 'border-transparent text-zinc-500 hover:bg-white/70 hover:text-zinc-800',
              )}
            >
              <Box className="h-3.5 w-3.5 shrink-0 opacity-90" aria-hidden />
              <span className="leading-none">Arch</span>
            </button>
            <button
              type="button"
              onClick={() => setActiveTab('pipelines')}
              className={cn(
                'flex flex-col items-center justify-center gap-1.5 border-b-2 py-3.5 text-[11px] font-medium transition-colors',
                activeTab === 'pipelines' ? 'border-zinc-900 bg-white text-zinc-900' : 'border-transparent text-zinc-500 hover:bg-white/70 hover:text-zinc-800',
              )}
            >
              <Workflow className="h-3.5 w-3.5 shrink-0 opacity-90" aria-hidden />
              <span className="leading-none">CI</span>
            </button>
            <button
              type="button"
              onClick={() => setActiveTab('infrastructure')}
              className={cn(
                'flex flex-col items-center justify-center gap-1.5 border-b-2 py-3.5 text-[11px] font-medium transition-colors',
                activeTab === 'infrastructure' ? 'border-zinc-900 bg-white text-zinc-900' : 'border-transparent text-zinc-500 hover:bg-white/70 hover:text-zinc-800',
              )}
            >
              <Server className="h-3.5 w-3.5 shrink-0 opacity-90" aria-hidden />
              <span className="leading-none">Infra</span>
            </button>
          </div>

          <div className="h-full overflow-y-auto px-4 py-6">
            {activeTab === 'graph' ? (
              <div className="space-y-4">
                <div className="rounded-xl border border-zinc-200/90 bg-zinc-50/50 p-4 shadow-sm">
                  <p className="mb-3 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">Flow mode</p>
                  <div className="flex gap-2">
                    <Button size="sm" variant={flowMode === 'route' ? 'default' : 'outline'} className="flex-1" onClick={() => setFlowMode('route')}>
                      Route
                    </Button>
                    <Button size="sm" variant={flowMode === 'ui' ? 'default' : 'outline'} className="flex-1" onClick={() => setFlowMode('ui')}>
                      UI
                    </Button>
                  </div>
                </div>

                <div className="relative">
                  <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-zinc-400" />
                  <Input
                    placeholder={flowMode === 'route' ? 'Search routes…' : 'Search components…'}
                    value={flowAnchorQuery}
                    onChange={(e) => setFlowAnchorQuery(e.target.value)}
                    className="h-10 rounded-lg border-zinc-200 bg-white pl-10 pr-3 text-sm text-zinc-900 shadow-sm placeholder:text-zinc-400 focus-visible:border-zinc-300 focus-visible:ring-2 focus-visible:ring-zinc-200/80"
                  />
                </div>

                <div className="flex items-center justify-between gap-2">
                  <p className="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
                    {flowMode === 'route' ? 'Routes' : 'Components'}
                  </p>
                  <div className="flex items-center gap-2 text-[11px] text-zinc-500 tabular-nums">
                    {anchorLoading && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
                    <span>
                      {graphTabAnchors.total.toLocaleString()}
                      {graphTabAnchors.capped ? '+' : ''}
                    </span>
                  </div>
                </div>

                <div className="overflow-hidden rounded-xl border border-zinc-200/90 bg-white shadow-sm">
                  <div className="max-h-[60vh] overflow-y-auto p-2">
                    {flowMode === 'route'
                      ? (graphTabAnchors.items as BackendFlowListItem[]).map((r) => {
                          const method = (r.method || '').toUpperCase();
                          const path = r.path || '';
                          const title = `${method} ${path}`.trim();
                          const key = `route:${method}:${path}`;
                          const isActive = selectedAnchorKey === key;
                          return (
                            <button
                              key={key}
                              type="button"
                              onClick={() => onSelectBackendAnchor(r)}
                              className={cn('w-full rounded-lg px-2.5 py-2 text-left transition-colors', isActive ? 'bg-zinc-100' : 'hover:bg-zinc-50')}
                            >
                              <div className="truncate text-xs font-medium text-zinc-900">{title}</div>
                            </button>
                          );
                        })
                      : (graphTabAnchors.items as FrontendFlowListItem[]).map((c, idx) => {
                          const name = c.name || '';
                          const title = name.trim() || '(unnamed component)';
                          const disambiguator = (c.qualified_name || c.file_path || c.selector || '').trim();
                          // Guarantee uniqueness even when backend omits file/qualified_name.
                          const key = `ui:${name}\0${disambiguator || String(idx)}`;
                          const isActive = selectedAnchorKey === key;
                          return (
                            <button
                              key={key}
                              type="button"
                              onClick={() => onSelectFrontendAnchor(c, key)}
                              className={cn('w-full rounded-lg px-2.5 py-2 text-left transition-colors', isActive ? 'bg-zinc-100' : 'hover:bg-zinc-50')}
                            >
                              <div className="truncate text-xs font-medium text-zinc-900">{title}</div>
                            </button>
                          );
                        })}
                  </div>
                  {graphTabAnchors.capped && (
                    <div className="border-t border-zinc-200 bg-zinc-50/50 px-3 py-2 text-[11px] text-zinc-500">
                      Showing first 200 results. Refine search to narrow.
                    </div>
                  )}
                </div>
              </div>
            ) : activeTab === 'architecture' ? (
              <div className="space-y-4">
                <div className="rounded-xl border border-zinc-200/90 bg-zinc-50/50 p-4 shadow-sm">
                  <p className="mb-3 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">Architecture</p>
                  <label htmlFor="arch-depth" className="block text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
                    Depth
                  </label>
                  <Select value={String(archDepth)} onValueChange={(v) => setArchDepth((Number(v) as 1 | 2 | 3) || 1)}>
                    <SelectTrigger id="arch-depth" className="mt-2 h-10 w-full rounded-lg border-zinc-200 bg-white text-sm shadow-sm">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="1" className="text-sm">1</SelectItem>
                      <SelectItem value="2" className="text-sm">2</SelectItem>
                      <SelectItem value="3" className="text-sm">3</SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                {selectedArchNode ? (
                  <div className="rounded-xl border border-zinc-200/90 bg-white p-4 shadow-sm">
                    <p className="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">Selected</p>
                    <p className="mt-2 text-sm font-medium text-zinc-900">{selectedArchNode.name}</p>
                    {selectedArchNode.file_path ? <p className="mt-1 text-xs font-mono text-zinc-500">{selectedArchNode.file_path}</p> : null}
                    <div className="mt-3 flex gap-2">
                      <Button size="sm" variant="outline" onClick={() => setSelectedArchNode(null)}>
                        Clear
                      </Button>
                      <Button size="sm" variant="outline" onClick={() => void loadLayoutData()}>
                        Refresh
                      </Button>
                    </div>
                  </div>
                ) : null}
              </div>
            ) : activeTab === 'pipelines' ? (
              <div className="space-y-3">
                <div className="flex items-center justify-between">
                  <p className="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">Pipelines</p>
                  {pipelineLoading ? <Loader2 className="h-4 w-4 animate-spin text-zinc-400" /> : null}
                </div>
                {pipelineList.map((p) => (
                  <button
                    key={`${p.source_project ?? ''}\0${p.pipeline.name}`}
                    type="button"
                    onClick={() => void selectPipeline(p)}
                    className={cn(
                      'w-full rounded-lg border border-zinc-200 bg-white p-3 text-left hover:bg-zinc-50',
                      selectedPipeline === p && 'ring-1 ring-zinc-200/80',
                    )}
                  >
                    <div className="text-sm font-medium text-zinc-900">{p.pipeline.name}</div>
                    <div className="mt-1 text-xs font-mono text-zinc-500">{p.pipeline.file_path}</div>
                  </button>
                ))}
              </div>
            ) : (
              <div className="space-y-3">
                <div className="flex items-center justify-between">
                  <p className="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">Infrastructure</p>
                  {infraLoading ? <Loader2 className="h-4 w-4 animate-spin text-zinc-400" /> : null}
                </div>
                <label className="flex items-center gap-2 text-xs text-zinc-600">
                  <input type="checkbox" checked={includeLinked} onChange={(e) => setIncludeLinked(e.target.checked)} />
                  Include linked projects
                </label>
                {infrastructure.slice(0, 40).map((r) => (
                  <div key={`${r.source_project ?? ''}\0${r.file_path}\0${r.name}`} className="rounded-lg border border-zinc-200 bg-white p-3">
                    <div className="text-sm font-medium text-zinc-900">{r.name}</div>
                    <div className="mt-1 text-xs font-mono text-zinc-500">{r.file_path}</div>
                  </div>
                ))}
              </div>
            )}
          </div>
        </aside>

        {/* Main canvas area */}
        <div className="relative flex min-h-0 flex-1 overflow-hidden bg-zinc-100">
          {activeTab === 'graph' ? (
            <div className="flex h-full w-full min-h-0">
              <div className="flex min-w-0 flex-1 flex-col gap-3 p-4">
                <div className="flex items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-medium text-zinc-900">
                      {selectedAnchorTitle ? selectedAnchorTitle : 'Select an anchor to explore'}
                    </div>
                    <div className="truncate text-xs text-zinc-500">
                      mode: {flowMode}
                      {typeof flowConfidence === 'number'
                        ? ` · confidence: ${Math.round((flowConfidence > 1 ? flowConfidence / 100 : flowConfidence) * 100)}%`
                        : ''}
                    </div>
                  </div>
                </div>

                <div className={cn('flex min-h-0 flex-1 transition-opacity duration-150 ease-out motion-reduce:transition-none', sankeyLoading ? 'opacity-70' : 'opacity-100')}>
                  <FlowSankeyView graph={sankeyGraph} selectedNodeId={selectedSankeyNodeId} onSelectNode={setSelectedSankeyNodeId} className="flex-1" />
                </div>
              </div>

              <OnboardingExtract
                mode={flowMode}
                anchorTitle={selectedAnchorTitle}
                confidence={flowConfidence}
                graph={sankeyGraph}
                flowNodesBySankeyId={flowNodesBySankeyId}
                className="w-[380px] shrink-0"
              />

              <div
                className={cn(
                  'pointer-events-none absolute inset-0 z-20 flex items-start justify-center pt-10',
                  'bg-white/40 transition-opacity duration-150 ease-out motion-reduce:transition-none',
                  sankeyLoading ? 'opacity-100' : 'opacity-0',
                )}
              >
                <div className="pointer-events-auto flex items-center gap-2 rounded-full border border-zinc-200/90 bg-white/95 px-3 py-1.5 shadow-lg">
                  <Loader2 className="h-3.5 w-3.5 animate-spin text-violet-600" />
                  <span className="text-[11px] font-medium text-zinc-600">Loading flow…</span>
                </div>
              </div>
            </div>
          ) : activeTab === 'architecture' ? (
            <div className="h-full w-full">
              <CanvasGraphView
                project={project}
                kind={`arch:${archDepth}`}
                data={architectureData}
                maxNodes={5000}
                selectedNodeId={selectedArchNode?.id ?? null}
                onSelectNode={setSelectedArchNode}
                className="h-full w-full"
              />
              {loading ? (
                <div className="absolute inset-0 z-20 flex items-center justify-center bg-white/70 backdrop-blur-[2px]">
                  <div className="flex items-center gap-2 rounded-full border border-zinc-200 bg-white px-3 py-1.5 shadow">
                    <Loader2 className="h-4 w-4 animate-spin text-blue-600" />
                    <span className="text-xs text-zinc-600">Loading graph…</span>
                  </div>
                </div>
              ) : null}
              {loadError ? (
                <div className="absolute inset-0 z-20 flex items-center justify-center bg-white/70 backdrop-blur-[2px]">
                  <div className="flex max-w-sm flex-col items-center gap-2 text-center">
                    <AlertCircle className="h-6 w-6 text-red-600" />
                    <p className="text-sm text-red-700">{loadError}</p>
                    <Button size="sm" variant="outline" onClick={() => void loadLayoutData()}>
                      Retry
                    </Button>
                  </div>
                </div>
              ) : null}
            </div>
          ) : activeTab === 'pipelines' ? (
            <PipelineDagView dag={selectedPipelineDag || selectedPipeline} />
          ) : (
            <div className="h-full w-full overflow-auto p-6">
              <p className="text-sm text-zinc-600">Select an infra item from the left.</p>
            </div>
          )}
        </div>
      </div>
    </main>
  );
}

