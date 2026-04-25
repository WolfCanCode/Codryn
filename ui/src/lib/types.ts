export interface GraphNode {
  id: number; x: number; y: number; label: string; name: string;
  file_path?: string; size: number; color: string;
}
export interface GraphEdge { source: number; target: number; type: string; }
export interface GraphData { nodes: GraphNode[]; edges: GraphEdge[]; total_nodes: number; }
export interface Project { name: string; root_path: string; indexed_at: string; }
export interface SchemaInfo {
  node_labels: { label: string; count: number }[];
  edge_types: { edge_type: string; count: number }[];
  total_nodes: number; total_edges: number;
}
export interface ToolCount { tool_name: string; count: number; avg_ms: number; mcp_count: number; ui_count: number; }
export interface SourceCount { source: string; count: number; }
export interface AgentCount { agent_name: string; count: number; }
export interface ModelCount { model_name: string; count: number; }
export interface ToolCall {
  id: number; tool_name: string; project: string; source: string;
  duration_ms: number; success: boolean; called_at: string;
  agent_name: string; model_name: string;
  input_tokens: number; output_tokens: number; response_bytes: number;
  request_body?: string; response_body?: string;
}
export interface Analytics {
  total_calls: number; per_tool: ToolCount[]; per_source: SourceCount[];
  per_agent: AgentCount[]; per_model: ModelCount[];
  total_input_tokens: number; total_output_tokens: number; total_response_bytes: number;
  estimated_tokens_used: number; estimated_tokens_without_tools: number; estimated_tokens_saved: number;
  recent: ToolCall[];
}
export interface PipelineDag {
  source_project?: string;
  pipeline: PipelineInfo;
  stages: StageInfo[];
  jobs: JobInfo[];
  edges: DagEdge[];
}
export interface PipelineInfo {
  name: string;
  file_path: string;
  ci_system: string;
  triggers: string[];
}
export interface StageInfo {
  name: string;
  order: number;
}
export interface JobInfo {
  name: string;
  stage: string;
  image?: string;
  dependencies: string[];
}
export interface DagEdge {
  source: string;
  target: string;
  edge_type: string;
}
export interface InfraResource {
  source_project?: string;
  name: string;
  resource_type: string;
  kind: string;
  file_path: string;
  properties: Record<string, unknown>;
}
