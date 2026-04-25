import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from '@/components/ui/accordion';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Card } from '@/components/ui/card';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { callTool } from '@/lib/rpc';
import {
  Download, Trash2, Folder, Play, RefreshCw, CheckCircle, XCircle, Loader2, ChevronUp, Search,
} from 'lucide-react';

interface Agent {
  name: string;
  installed: boolean;
  configured: boolean;
  config_path: string;
  has_instructions: boolean;
  instructions_path: string;
}

interface Doctor {
  codryn_version: string;
  codryn_binary: string;
  store_path: string;
  store_exists: boolean;
  agents: Agent[];
}

interface ProjectStatus {
  project: string;
  total_nodes: number;
  total_edges: number;
}

interface Storage {
  graph_db_human: string;
  binary_human: string;
}

type ToolError = { error?: string };
type ListProjectsResult = { projects?: Array<{ name?: string; project?: string }> } & ToolError;
type GraphSchemaResult = { total_nodes?: number; total_edges?: number } & ToolError;
type IndexRepositoryResult = { project?: string; error?: string };
type BrowseResult = { error?: string; path?: string; parent?: string; dirs?: string[] };

const TEMPLATES: Record<string, string> = {
  'All functions': 'MATCH (f:Function) RETURN f.name, f.file_path LIMIT 20',
  'Call chains': 'MATCH (a:Function)-[:CALLS]->(b:Function) RETURN a.name, b.name LIMIT 30',
  'Who calls…': "MATCH (c:Function)-[:CALLS]->(f:Function) WHERE f.name = 'FUNCTION_NAME' RETURN c.name",
  'Classes': 'MATCH (c:Class) RETURN c.name, c.file_path LIMIT 20',
  'Imports': 'MATCH (a)-[:IMPORTS]->(b) RETURN a.name, b.name LIMIT 20',
  'Inheritance': 'MATCH (a:Class)-[:INHERITS]->(b:Class) RETURN a.name, b.name LIMIT 20',
  'Node count': 'MATCH (n:Function) RETURN COUNT(n)',
  'Files': 'MATCH (f:File) RETURN f.name, f.file_path LIMIT 30',
};

const Check = () => <span className="text-green-500 font-bold">✓</span>;
const Dash = () => <span className="text-gray-400">–</span>;

export default function ConfigPage() {
  const navigate = useNavigate();

  const [doctor, setDoctor] = useState<Doctor | null>(null);
  const [installing, setInstalling] = useState(false);
  const [uninstalling, setUninstalling] = useState(false);
  const [installResult, setInstallResult] = useState<{ message?: string; error?: string } | null>(null);

  const [repoPath, setRepoPath] = useState('');
  const [indexing, setIndexing] = useState(false);
  const [indexResult, setIndexResult] = useState<{ project?: string; error?: string } | null>(null);
  const [browseOpen, setBrowseOpen] = useState(false);
  const [browsePath, setBrowsePath] = useState('');
  const [browseParent, setBrowseParent] = useState('');
  const [browseDirs, setBrowseDirs] = useState<string[]>([]);

  const [cypherQuery, setCypherQuery] = useState('');
  const [cypherProject, setCypherProject] = useState('');
  const [querying, setQuerying] = useState(false);
  const [queryResult, setQueryResult] = useState<unknown>(null);

  const [projectStatuses, setProjectStatuses] = useState<ProjectStatus[]>([]);
  const [projectNames, setProjectNames] = useState<string[]>([]);
  const [statusLoaded, setStatusLoaded] = useState(false);
  const [storage, setStorage] = useState<Storage | null>(null);

  useEffect(() => {
    loadDoctor();
    refreshStatus();
  }, []);

  async function loadDoctor() {
    try {
      const res = await fetch('/api/doctor');
      setDoctor(await res.json());
    } catch {
      setDoctor(null);
    }
  }

  async function refreshStatus() {
    try {
      const result = await callTool<ListProjectsResult>('list_projects', {});
      const projects = result?.projects ?? [];
      const names: string[] = projects.map((p) => p.name ?? p.project).filter((n): n is string => Boolean(n));
      setProjectNames(names);

      const statuses: ProjectStatus[] = [];
      for (const name of names) {
        try {
          const schema = await callTool<GraphSchemaResult>('get_graph_schema', { project: name });
          statuses.push({
            project: name,
            total_nodes: schema?.total_nodes ?? 0,
            total_edges: schema?.total_edges ?? 0,
          });
        } catch {
          statuses.push({ project: name, total_nodes: 0, total_edges: 0 });
        }
      }
      setProjectStatuses(statuses);
      setStatusLoaded(true);
    } catch {
      setProjectStatuses([]);
      setProjectNames([]);
      setStatusLoaded(true);
    }

    try {
      const res = await fetch('/api/storage');
      setStorage(await res.json());
    } catch {
      setStorage(null);
    }
  }

  async function handleInstall() {
    setInstalling(true);
    setInstallResult(null);
    try {
      const res = await fetch('/api/install', { method: 'POST' });
      const data = await res.json();
      setInstallResult(res.ok ? { message: data.message ?? 'Installed' } : { error: data.error ?? 'Failed' });
      loadDoctor();
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setInstallResult({ error: msg });
    } finally {
      setInstalling(false);
    }
  }

  async function handleUninstall() {
    setUninstalling(true);
    setInstallResult(null);
    try {
      const res = await fetch('/api/uninstall', { method: 'POST' });
      const data = await res.json();
      setInstallResult(res.ok ? { message: data.message ?? 'Uninstalled' } : { error: data.error ?? 'Failed' });
      loadDoctor();
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setInstallResult({ error: msg });
    } finally {
      setUninstalling(false);
    }
  }

  function browseDirName(dirPath: string, basePath: string): string {
    const sep = basePath.includes('\\') ? '\\' : '/';
    const root = basePath.replace(/[/\\]+$/, '');
    return `${root}${sep}${dirPath}`;
  }

  async function browse(path?: string) {
    try {
      const q = path ? `?path=${encodeURIComponent(path)}` : '';
      const res = await fetch(`/api/browse${q}`);
      const data = (await res.json()) as BrowseResult;
      if (data.error) return;
      setBrowsePath(data.path ?? '');
      setBrowseParent(data.parent ?? '');
      setBrowseDirs(data.dirs ?? []);
      setBrowseOpen(true);
    } catch {
      setBrowseOpen(false);
    }
  }

  /** Open a row: API returns full paths; if we only have a label, join with current browse path. */
  function openBrowseEntry(entry: string) {
    const isAbsolute =
      entry.startsWith('/') || /^[a-zA-Z]:[\\/]/.test(entry) || entry.startsWith('\\\\');
    const target = isAbsolute ? entry : browseDirName(entry, browsePath);
    void browse(target);
  }

  async function handleIndex() {
    setIndexing(true);
    setIndexResult(null);
    try {
      const result = await callTool<IndexRepositoryResult>('index_repository', { path: repoPath });
      setIndexResult(result?.error ? { error: result.error } : { project: result?.project ?? repoPath });
      refreshStatus();
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setIndexResult({ error: msg });
    } finally {
      setIndexing(false);
    }
  }

  async function handleQuery() {
    setQuerying(true);
    setQueryResult(null);
    try {
      const args: { query: string; project?: string } = { query: cypherQuery };
      if (cypherProject) args.project = cypherProject;
      setQueryResult(await callTool<unknown>('query_graph', args));
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setQueryResult({ error: msg });
    } finally {
      setQuerying(false);
    }
  }

  return (
    <div className="w-full max-w-[800px] space-y-4 p-6 text-left">
      <h2 className="text-xl font-semibold tracking-tight text-foreground">Configuration</h2>

      <Accordion multiple defaultValue={['tooling']} className="space-y-2">
        {/* 1. Tooling Status */}
        <AccordionItem value="tooling">
          <AccordionTrigger>Tooling Status</AccordionTrigger>
          <AccordionContent>
            {doctor ? (
              <div className="space-y-4">
                <div className="grid grid-cols-3 gap-4 text-sm">
                  <div>
                    <span className="text-muted-foreground">Version</span>
                    <p className="font-mono">{doctor.codryn_version}</p>
                  </div>
                  <div>
                    <span className="text-muted-foreground">Binary</span>
                    <p className="font-mono text-xs break-all">{doctor.codryn_binary}</p>
                  </div>
                  <div>
                    <span className="text-muted-foreground">Store</span>
                    <p className="font-mono text-xs break-all">
                      {doctor.store_path} {doctor.store_exists ? <Check /> : <XCircle className="inline h-4 w-4 text-red-500" />}
                    </p>
                  </div>
                </div>

                <div className="border rounded-md overflow-hidden">
                  <table className="w-full text-sm">
                    <thead className="bg-muted">
                      <tr>
                        <th className="text-left p-2">Agent</th>
                        <th className="text-center p-2">Installed</th>
                        <th className="text-center p-2">Configured</th>
                        <th className="text-center p-2">Instructions</th>
                      </tr>
                    </thead>
                    <tbody>
                      {doctor.agents.map((a) => (
                        <tr key={a.name} className="border-t">
                          <td className="p-2 font-medium">{a.name}</td>
                          <td className="text-center p-2">{a.installed ? <Check /> : <Dash />}</td>
                          <td className="text-center p-2">{a.configured ? <Check /> : <Dash />}</td>
                          <td className="text-center p-2">{a.has_instructions ? <Check /> : <Dash />}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>

                <div className="flex gap-2">
                  <Button onClick={handleInstall} disabled={installing}>
                    {installing ? <Loader2 className="h-4 w-4 animate-spin mr-2" /> : <Download className="h-4 w-4 mr-2" />}
                    Install
                  </Button>
                  <Button variant="destructive" onClick={handleUninstall} disabled={uninstalling}>
                    {uninstalling ? <Loader2 className="h-4 w-4 animate-spin mr-2" /> : <Trash2 className="h-4 w-4 mr-2" />}
                    Uninstall
                  </Button>
                </div>

                {installResult && (
                  <Alert variant={installResult.error ? 'destructive' : 'default'}>
                    <AlertDescription>
                      {installResult.error ? (
                        <><XCircle className="inline h-4 w-4 mr-1" />{installResult.error}</>
                      ) : (
                        <><CheckCircle className="inline h-4 w-4 mr-1 text-green-500" />{installResult.message}</>
                      )}
                    </AlertDescription>
                  </Alert>
                )}
              </div>
            ) : (
              <p className="text-muted-foreground text-sm">Loading…</p>
            )}
          </AccordionContent>
        </AccordionItem>

        {/* 2. Index Repository */}
        <AccordionItem value="index">
          <AccordionTrigger>Index Repository</AccordionTrigger>
          <AccordionContent>
            <div className="space-y-3">
              <Alert>
                <AlertDescription>
                  For monorepos, index the <strong>repo root</strong> (the folder that contains <code>.github/</code>) so CI and shared infra files are recognized.
                  You can still index sub-projects too, then link them from the Projects page to view CI/Infra across projects.
                </AlertDescription>
              </Alert>
              <div className="flex gap-2">
                <Input
                  placeholder="Repository path (e.g. /home/user/project)"
                  value={repoPath}
                  onChange={(e) => setRepoPath(e.target.value)}
                />
                <Button variant="outline" onClick={() => browse(repoPath || undefined)}>
                  <Folder className="h-4 w-4" />
                </Button>
                <Button onClick={handleIndex} disabled={indexing || !repoPath}>
                  {indexing ? <Loader2 className="h-4 w-4 animate-spin mr-2" /> : <Play className="h-4 w-4 mr-2" />}
                  Index
                </Button>
              </div>

              {browseOpen && (
                <div className="border rounded-md p-3 space-y-2 bg-muted/50">
                  <p className="text-xs font-mono break-all">{browsePath}</p>
                  <div className="max-h-48 overflow-y-auto space-y-1">
                    {browseParent && (
                      <button type="button" className="text-sm text-blue-500 hover:underline flex items-center gap-1" onClick={() => browse(browseParent)}>
                        <ChevronUp className="h-3 w-3" /> ..
                      </button>
                    )}
                    {browseDirs.map((d) => {
                      const label = d.replace(/[/\\]+$/, '').split(/[/\\]/).pop() ?? d;
                      return (
                        <button
                          key={d}
                          type="button"
                          className="flex w-full items-center gap-2 rounded px-1 py-0.5 text-left text-sm hover:bg-muted hover:text-blue-600"
                          onClick={() => openBrowseEntry(d)}
                        >
                          <Folder className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          <span>{label}</span>
                        </button>
                      );
                    })}
                  </div>
                  <div className="flex gap-2 pt-1">
                    <Button size="sm" onClick={() => { setRepoPath(browsePath); setBrowseOpen(false); }}>Select</Button>
                    <Button size="sm" variant="outline" onClick={() => setBrowseOpen(false)}>Cancel</Button>
                  </div>
                </div>
              )}

              {indexResult && (
                <Alert variant={indexResult.error ? 'destructive' : 'default'}>
                  <AlertDescription>
                    {indexResult.error ? (
                      <><XCircle className="inline h-4 w-4 mr-1" />{indexResult.error}</>
                    ) : (
                      <span className="flex items-center gap-2">
                        <CheckCircle className="h-4 w-4 text-green-500" />
                        Indexed <strong>{indexResult.project}</strong>
                        <Button
                          size="sm"
                          variant="link"
                          onClick={() =>
                            navigate('/graph?project=' + encodeURIComponent(indexResult.project ?? ''))
                          }
                        >
                          Open graph
                        </Button>
                      </span>
                    )}
                  </AlertDescription>
                </Alert>
              )}
            </div>
          </AccordionContent>
        </AccordionItem>

        {/* 3. Cypher Query Console */}
        <AccordionItem value="cypher">
          <AccordionTrigger>Cypher Query Console</AccordionTrigger>
          <AccordionContent>
            <div className="space-y-3">
              <div className="flex flex-wrap gap-1">
                {Object.entries(TEMPLATES).map(([label, query]) => (
                  <Button key={label} size="sm" variant="secondary" className="text-xs" onClick={() => setCypherQuery(query)}>
                    {label}
                  </Button>
                ))}
              </div>

              <Textarea
                className="font-mono text-sm"
                rows={4}
                placeholder="MATCH (n) RETURN n LIMIT 10"
                value={cypherQuery}
                onChange={(e) => setCypherQuery(e.target.value)}
              />

              <details className="text-xs text-muted-foreground">
                <summary className="cursor-pointer">Syntax reference</summary>
                <pre className="mt-1 p-2 bg-muted rounded text-xs whitespace-pre-wrap">
{`MATCH (n:Label)                  — match nodes by label
MATCH (a)-[:EDGE]->(b)           — match edges by type
WHERE n.name = 'value'           — filter
RETURN n.name, n.file_path       — select properties
LIMIT 20                         — limit results
COUNT(n)                         — aggregate

Labels: Function, Class, Method, Module, File, Folder, Interface, Route, Selector
Edges: CALLS, IMPORTS, INHERITS, IMPLEMENTS, CONTAINS, USES, HANDLES_ROUTE, RENDERS, INJECTS`}
                </pre>
              </details>

              <div className="flex gap-2">
                <Select value={cypherProject} onValueChange={v => setCypherProject(v ?? '')}>
                  <SelectTrigger className="w-48">
                    <SelectValue placeholder="All projects" />
                  </SelectTrigger>
                  <SelectContent>
                    {projectNames.map((n) => (
                      <SelectItem key={n} value={n}>{n}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Button onClick={handleQuery} disabled={querying || !cypherQuery}>
                  {querying ? <Loader2 className="h-4 w-4 animate-spin mr-2" /> : <Search className="h-4 w-4 mr-2" />}
                  Execute
                </Button>
              </div>

              {queryResult != null ? (
                <Card className="p-3">
                  <pre className="text-xs overflow-auto max-h-64 whitespace-pre-wrap">
                    {JSON.stringify(queryResult, null, 2)}
                  </pre>
                </Card>
              ) : null}
            </div>
          </AccordionContent>
        </AccordionItem>

        {/* 4. System Status */}
        <AccordionItem value="status">
          <AccordionTrigger>System Status</AccordionTrigger>
          <AccordionContent>
            <div className="space-y-3">
              <Button size="sm" variant="outline" onClick={refreshStatus}>
                <RefreshCw className="h-4 w-4 mr-2" /> Refresh
              </Button>

              {statusLoaded ? (
                projectStatuses.length > 0 ? (
                  <div className="border rounded-md overflow-hidden">
                    <table className="w-full text-sm">
                      <thead className="bg-muted">
                        <tr>
                          <th className="text-left p-2">Project</th>
                          <th className="text-right p-2">Nodes</th>
                          <th className="text-right p-2">Edges</th>
                        </tr>
                      </thead>
                      <tbody>
                        {projectStatuses.map((s) => (
                          <tr key={s.project} className="border-t">
                            <td className="p-2 font-medium">{s.project}</td>
                            <td className="text-right p-2 font-mono">{s.total_nodes.toLocaleString()}</td>
                            <td className="text-right p-2 font-mono">{s.total_edges.toLocaleString()}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                ) : (
                  <p className="text-sm text-muted-foreground">No projects indexed yet.</p>
                )
              ) : (
                <p className="text-sm text-muted-foreground">Loading…</p>
              )}

              {storage && (
                <div className="flex gap-6 text-sm">
                  <div>
                    <span className="text-muted-foreground">graph.db</span>
                    <p className="font-mono">{storage.graph_db_human}</p>
                  </div>
                  <div>
                    <span className="text-muted-foreground">Binary</span>
                    <p className="font-mono">{storage.binary_human}</p>
                  </div>
                </div>
              )}
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </div>
  );
}
