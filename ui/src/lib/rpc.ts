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

export async function fetchRoutes(project: string) {
  const res = await fetch(`/api/backend-flow?project=${encodeURIComponent(project)}&list=true`);
  return res.json();
}

export async function fetchBackendFlow(project: string, routePath: string, httpMethod?: string) {
  let url = `/api/backend-flow?project=${encodeURIComponent(project)}&route_path=${encodeURIComponent(routePath)}`;
  if (httpMethod) url += `&http_method=${encodeURIComponent(httpMethod)}`;
  const res = await fetch(url);
  return res.json();
}

export async function fetchComponents(project: string) {
  const res = await fetch(`/api/frontend-flow?project=${encodeURIComponent(project)}&list=true`);
  return res.json();
}

export async function fetchFrontendFlow(project: string, component: string) {
  const res = await fetch(`/api/frontend-flow?project=${encodeURIComponent(project)}&component=${encodeURIComponent(component)}`);
  return res.json();
}
