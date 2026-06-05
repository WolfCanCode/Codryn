use serde::{Deserialize, Serialize};

/// Checkpoint record stored in `_index_progress` table for crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexCheckpoint {
    pub project: String,
    pub phase: String,
    pub phase_index: u32,
    pub files_processed: u32,
    pub started_at: String,
    pub completed: bool,
    /// Optional index run ID linking this checkpoint to an `_index_runs` record.
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: i64,
    pub project: String,
    pub label: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub properties_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: i64,
    pub project: String,
    pub source_id: i64,
    pub target_id: i64,
    pub edge_type: String,
    pub properties_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub indexed_at: String,
    pub root_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHash {
    pub project: String,
    pub rel_path: String,
    pub sha256: String,
    pub mtime_ns: i64,
    pub size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectLink {
    pub source_project: String,
    pub target_project: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adr {
    pub id: String,
    pub project: String,
    pub title: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: i64,
    pub tool_name: String,
    pub project: String,
    pub source: String,
    pub duration_ms: i64,
    pub success: bool,
    pub called_at: String,
    pub agent_name: String,
    pub model_name: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub response_bytes: i64,
    #[serde(default)]
    pub request_body: String,
    #[serde(default)]
    pub response_body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAnalytics {
    pub total_calls: i64,
    pub per_tool: Vec<ToolCount>,
    pub per_agent: Vec<AgentCount>,
    pub per_model: Vec<ModelCount>,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_response_bytes: i64,
    pub estimated_tokens_used: i64,
    pub estimated_tokens_without_tools: i64,
    pub estimated_tokens_saved: i64,
    pub recent: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCount {
    pub tool_name: String,
    pub count: i64,
    pub avg_ms: f64,
    pub mcp_count: i64,
    pub ui_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCount {
    pub agent_name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCount {
    pub model_name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaInfo {
    pub node_labels: Vec<LabelCount>,
    pub edge_types: Vec<TypeCount>,
    pub total_nodes: i64,
    pub total_edges: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelCount {
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeCount {
    pub edge_type: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetadataFilter {
    pub is_test: Option<bool>,
    pub is_exported: Option<bool>,
    pub is_entry_point: Option<bool>,
    pub min_complexity: Option<u32>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteInfo {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub qualified_name: String,
    pub file_path: String,
    pub controller: String,
    pub request_dto: Option<String>,
    pub response_dto: Option<String>,
    pub score: f64,
    pub extraction_confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A single row returned by the complexity query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityRow {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i32,
    pub cyclomatic_complexity: u32,
    pub cognitive_complexity: u32,
}

/// A row returned by the hotspot (git history) query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotspotRow {
    pub name: String,
    pub qualified_name: String,
    pub label: String,
    pub file_path: String,
    pub start_line: i32,
    pub git_commits: i64,
    pub git_authors: i64,
    pub git_last_modified: String,
}

/// A row returned by the doc coverage query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocCoverageRow {
    pub module: String,
    pub total_symbols: u32,
    pub documented_symbols: u32,
    pub coverage_pct: f64,
    /// True if coverage is below 50%.
    pub needs_attention: bool,
}
