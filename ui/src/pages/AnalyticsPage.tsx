import { useEffect, useState, useMemo } from 'react';
import { Button } from '@/components/ui/button';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { Loader2, RefreshCw } from 'lucide-react';
import type { Analytics, ToolCount, ToolCall } from '@/lib/types';

const TOOL_COLORS: Record<string, string> = {
  list_projects: '#f9a825', get_graph_schema: '#00838f', index_repository: '#388e3c',
  search_graph: '#1976d2', query_graph: '#7b1fa2', index_status: '#ff7043',
  trace_call_path: '#e64a19', get_architecture: '#6a1b9a', get_code_snippet: '#546e7a',
  search_code: '#1565c0', detect_changes: '#8d6e63', delete_project: '#c62828',
  manage_adr: '#2e7d32', ingest_traces: '#0277bd', link_project: '#ad1457',
  list_project_links: '#00695c', search_linked_projects: '#4527a0',
  find_symbol: '#0097a7', get_symbol_details: '#558b2f', find_references: '#6d4c41',
  impact_analysis: '#d84315', explain_index_result: '#37474f',
};

const fmt = (n: number): string => {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
  if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
  return String(n);
};

const fmtB = (n: number): string => {
  if (n >= 1_048_576) return (n / 1_048_576).toFixed(1) + ' MB';
  if (n >= 1_024) return (n / 1_024).toFixed(1) + ' KB';
  return n + ' B';
};

const fmtT = (iso: string): string =>
  new Date(iso).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit', second: '2-digit' });

const tokenDisplay = (c: ToolCall): string => {
  if (c.input_tokens > 0) return `${fmt(c.input_tokens)}/${fmt(c.output_tokens)}`;
  if (c.response_bytes > 0) return `~${fmt(Math.round(c.response_bytes / 4))}`;
  return '—';
};

const clr = (name: string): string => TOOL_COLORS[name] ?? '#78909c';

export default function AnalyticsPage() {
  const [data, setData] = useState<Analytics | null>(null);

  const load = () => {
    setData(null);
    fetch('/api/analytics').then(r => r.json()).then(setData);
  };

  useEffect(load, []);

  const tools = useMemo(() =>
    data?.per_tool.filter((t: ToolCount) => t.mcp_count > 0).sort((a: ToolCount, b: ToolCount) => b.mcp_count - a.mcp_count) ?? [],
    [data]);

  const recent = useMemo(() =>
    data?.recent.filter((c: ToolCall) => c.source === 'mcp') ?? [],
    [data]);

  const maxCount = useMemo(() => Math.max(1, ...tools.map((t: ToolCount) => t.mcp_count)), [tools]);
  const pct = (n: number) => (n / maxCount) * 100;

  const avgDuration = useMemo(() => {
    if (!tools.length) return 0;
    const total = tools.reduce((s: number, t: ToolCount) => s + t.mcp_count, 0);
    return total ? Math.round(tools.reduce((s: number, t: ToolCount) => s + t.avg_ms * t.mcp_count, 0) / total) : 0;
  }, [tools]);

  const savingsPercent = useMemo(() => {
    if (!data || !data.estimated_tokens_without_tools) return 0;
    return Math.round((data.estimated_tokens_saved / data.estimated_tokens_without_tools) * 100);
  }, [data]);

  return (
    <div className="w-full max-w-6xl space-y-6 p-6 text-left">
      <div className="flex items-center justify-between gap-4">
        <h2 className="text-xl font-semibold tracking-tight text-foreground">Analytics</h2>
        <Button variant="outline" size="sm" onClick={load}><RefreshCw className="w-4 h-4 mr-1" />Refresh</Button>
      </div>

      {!data ? (
        <div className="flex justify-start py-16"><Loader2 className="h-8 w-8 animate-spin text-muted-foreground" /></div>
      ) : (
        <>
          {/* Overview Cards */}
          <div className="grid grid-cols-4 gap-3">
            {[
              { label: 'Total Calls', value: fmt(data.total_calls) },
              { label: 'Avg Response', value: `${avgDuration} ms` },
              { label: 'Tokens Saved', value: `${savingsPercent}%`, green: true },
              { label: 'Response Data', value: fmtB(data.total_response_bytes) },
            ].map(c => (
              <div key={c.label} className="bg-white border rounded-lg p-4">
                <div className="text-xs text-muted-foreground">{c.label}</div>
                <div className={`text-2xl font-bold mt-1 ${c.green ? 'text-green-600' : ''}`}>{c.value}</div>
              </div>
            ))}
          </div>

          {/* Agents & Models */}
          <div>
            <h3 className="text-sm font-medium mb-2">Agents &amp; Models</h3>
            <div className="flex items-center gap-2 flex-wrap text-xs">
              {data.per_source?.map((s: { source: string; count: number }) => (
                <span key={s.source} className={`px-2 py-0.5 rounded-full ${s.source === 'mcp' ? 'bg-blue-100 text-blue-700' : 'bg-pink-100 text-pink-700'}`}>
                  {s.source} ({s.count})
                </span>
              ))}
              <span className="text-gray-300">|</span>
              {data.per_agent?.map(a => (
                <span key={a.agent_name} className="px-2 py-0.5 rounded-full bg-green-100 text-green-700">{a.agent_name} ({a.count})</span>
              ))}
              <span className="text-gray-300">|</span>
              {data.per_model?.map(m => (
                <span key={m.model_name} className="px-2 py-0.5 rounded-full bg-purple-100 text-purple-700">{m.model_name} ({m.count})</span>
              ))}
            </div>
          </div>

          {/* Tools Bar Chart */}
          <div>
            <h3 className="text-sm font-medium">Tools <span className="text-muted-foreground font-normal">— agent calls only</span></h3>
            <div className="mt-2 space-y-1">
              {tools.map((t: ToolCount) => (
                <div key={t.tool_name} className="flex items-center gap-2 text-xs">
                  <div className="w-[160px] flex items-center gap-1.5 truncate">
                    <span className="w-2 h-2 rounded-full shrink-0" style={{ backgroundColor: clr(t.tool_name) }} />
                    {t.tool_name}
                  </div>
                  <div className="flex-1 bg-gray-100 rounded h-4">
                    <div className="h-full rounded" style={{ width: `${pct(t.mcp_count)}%`, backgroundColor: clr(t.tool_name) }} />
                  </div>
                  <div className="w-8 text-right font-medium">{t.mcp_count}</div>
                  <div className="w-12 text-right text-muted-foreground">{Math.round(t.avg_ms)}ms</div>
                </div>
              ))}
            </div>
          </div>

          {/* Recent Agent Calls */}
          <div>
            <h3 className="text-sm font-medium mb-2">Recent Agent Calls</h3>
            <div className="overflow-auto max-h-[400px] border rounded-lg">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Tool</TableHead>
                    <TableHead>Project</TableHead>
                    <TableHead>Agent</TableHead>
                    <TableHead>Model</TableHead>
                    <TableHead className="text-right">Duration</TableHead>
                    <TableHead className="text-right">Context</TableHead>
                    <TableHead className="text-right">Response</TableHead>
                    <TableHead>Time</TableHead>
                    <TableHead className="text-center">Status</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {recent.map((c: ToolCall, i: number) => (
                    <TableRow key={i} className="text-xs">
                      <TableCell className="font-medium">
                        <span className="inline-block w-2 h-2 rounded-full mr-1.5" style={{ backgroundColor: clr(c.tool_name) }} />
                        {c.tool_name}
                      </TableCell>
                      <TableCell>{c.project || '—'}</TableCell>
                      <TableCell>{c.agent_name || '—'}</TableCell>
                      <TableCell>{c.model_name || '—'}</TableCell>
                      <TableCell className="text-right">{c.duration_ms}ms</TableCell>
                      <TableCell className="text-right">{tokenDisplay(c)}</TableCell>
                      <TableCell className="text-right">{c.response_bytes > 0 ? fmtB(c.response_bytes) : '—'}</TableCell>
                      <TableCell>{fmtT(c.called_at)}</TableCell>
                      <TableCell className="text-center">{c.success ? <span className="text-green-600">✓</span> : <span className="text-red-600">✗</span>}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
