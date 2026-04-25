export type FlowNode = {
  id: string | number;
  name: string;
  label?: string;
  layer?: string;
  file_path?: string;
};

export type FlowEdge = {
  source: string | number | { id: string | number };
  target: string | number | { id: string | number };
  type?: string;
};

export type BackendFlowResponse = {
  nodes?: FlowNode[];
  edges?: FlowEdge[];
  summary?: { confidence?: number; flow_type?: string; renders?: number; injects?: number };
  error?: string;
};

export type FrontendFlowResponse = BackendFlowResponse;

export type BackendFlowListItem = {
  method: string;
  path: string;
  handler?: string;
  controller?: string;
};

export type BackendFlowListResponse = {
  routes?: BackendFlowListItem[];
  count?: number;
  error?: string;
};

export type FrontendFlowListItem = {
  name: string;
  selector?: string;
  file_path?: string;
  qualified_name?: string;
};

export type FrontendFlowListResponse = {
  components?: FrontendFlowListItem[];
  count?: number;
  error?: string;
};

export type SankeyLane =
  | 'route'
  | 'controller'
  | 'service'
  | 'repository'
  | 'dto'
  | 'component'
  | 'framework'
  | 'unknown';

export type SankeyNode = {
  id: string; // stable unique id
  title: string; // display label
  lane: SankeyLane;
  file_path?: string;
  rawNodeId?: string; // original flow node id (optional)
};

export type SankeyLink = {
  source: string;
  target: string;
  weight: number;
  edgeTypes: string[];
};

export type SankeyGraph = {
  nodes: SankeyNode[];
  links: SankeyLink[];
  lanes: SankeyLane[];
};
