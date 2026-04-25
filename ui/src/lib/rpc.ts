import type { InfraResource, PipelineDag, ToolCall } from './types';
import type {
  BackendFlowListResponse,
  BackendFlowResponse,
  FrontendFlowListResponse,
  FrontendFlowResponse,
} from '@/components/flow/flowTypes';

let nextId = 1;

interface JsonRpcResponse {
  result?: { content?: { text: string }[] };
  error?: { message: string };
}

export async function callTool<T = unknown>(name: string, args: Record<string, unknown> = {}): Promise<T> {
  const res = await fetch('/rpc', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: nextId++, method: 'tools/call', params: { name, arguments: args } }),
  });
  const json: JsonRpcResponse = await res.json();
  if (json.error) throw new Error(json.error.message ?? 'unknown error');
  const text = json?.result?.content?.[0]?.text;
  if (text === undefined) return json.result as T;
  return JSON.parse(text) as T;
}

export async function fetchLayout(project: string, maxNodes = 5000) {
  const res = await fetch(`/api/layout?project=${encodeURIComponent(project)}&max_nodes=${maxNodes}`);
  return res.json();
}

export async function fetchBackendFlowList(project: string): Promise<BackendFlowListResponse> {
  const res = await fetch(`/api/backend-flow?project=${encodeURIComponent(project)}&list=true`);
  return res.json();
}

export async function fetchFrontendFlowList(project: string): Promise<FrontendFlowListResponse> {
  const res = await fetch(`/api/frontend-flow?project=${encodeURIComponent(project)}&list=true`);
  return res.json();
}

export async function fetchBackendFlowGraph(
  project: string,
  routePath: string,
  httpMethod?: string,
): Promise<BackendFlowResponse> {
  let url = `/api/backend-flow?project=${encodeURIComponent(project)}&route_path=${encodeURIComponent(routePath)}`;
  if (httpMethod) url += `&http_method=${encodeURIComponent(httpMethod)}`;
  const res = await fetch(url);
  return res.json();
}

export async function fetchFrontendFlowGraph(project: string, component: string): Promise<FrontendFlowResponse> {
  const res = await fetch(`/api/frontend-flow?project=${encodeURIComponent(project)}&component=${encodeURIComponent(component)}`);
  return res.json();
}

// Back-compat for older callers
export async function fetchRoutes(project: string) {
  return fetchBackendFlowList(project);
}

export async function fetchBackendFlow(project: string, routePath: string, httpMethod?: string) {
  return fetchBackendFlowGraph(project, routePath, httpMethod);
}

export async function fetchComponents(project: string) {
  return fetchFrontendFlowList(project);
}

export async function fetchFrontendFlow(project: string, component: string) {
  return fetchFrontendFlowGraph(project, component);
}

export async function fetchPipelines(project: string, includeLinked = true): Promise<{ pipelines: PipelineDag[]; count: number }> {
  const res = await fetch(`/api/pipelines?project=${encodeURIComponent(project)}&include_linked=${includeLinked ? 'true' : 'false'}`);
  return res.json();
}

export async function fetchPipelineDag(project: string, name: string, sourceProject?: string, includeLinked = true): Promise<PipelineDag> {
  let url = `/api/pipelines?project=${encodeURIComponent(project)}&name=${encodeURIComponent(name)}&include_linked=${includeLinked ? 'true' : 'false'}`;
  if (sourceProject) url += `&source_project=${encodeURIComponent(sourceProject)}`;
  const res = await fetch(url);
  return res.json();
}

export async function fetchInfrastructure(project: string, type?: string, includeLinked = true): Promise<{ resources: InfraResource[]; count: number }> {
  let url = `/api/infrastructure?project=${encodeURIComponent(project)}&include_linked=${includeLinked ? 'true' : 'false'}`;
  if (type) url += `&type=${encodeURIComponent(type)}`;
  const res = await fetch(url);
  return res.json();
}

export async function fetchAnalyticsDetail(id: number): Promise<ToolCall> {
  const res = await fetch(`/api/analytics/${encodeURIComponent(id)}`);
  return res.json();
}
