use serde_json::Value;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AnalyticsMeta {
    #[schemars(description = "Agent identifier (e.g. kiro, claude-code, cursor)")]
    pub agent_name: Option<String>,
    #[schemars(description = "Model identifier (e.g. claude-sonnet-4.6)")]
    pub model_name: Option<String>,
    #[schemars(description = "Input/prompt tokens used so far")]
    pub input_tokens: Option<i64>,
    #[schemars(description = "Output/completion tokens generated so far")]
    pub output_tokens: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ProjectArg {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct IndexArgs {
    #[schemars(description = "Absolute path to the repository root")]
    pub path: String,
    #[schemars(description = "Index mode: full or fast")]
    pub mode: Option<String>,
    #[schemars(
        description = "If true, wipe all existing index data for this project before indexing (full rebuild from scratch). Use when the index is corrupted or you want a clean slate."
    )]
    pub clear_cache: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Search query string")]
    pub query: String,
    #[schemars(description = "Maximum results")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct QueryArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Cypher query string")]
    pub query: String,
    #[schemars(
        description = "If true, also run the query against all linked projects and tag results"
    )]
    pub include_linked: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TraceArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Source function name or qualified name")]
    pub source: String,
    #[schemars(description = "Target function name or qualified name")]
    pub target: String,
    #[schemars(description = "Maximum path depth")]
    pub max_depth: Option<i32>,
    #[schemars(description = "Minimum confidence threshold for filtering edges (0.0 to 1.0)")]
    pub min_confidence: Option<f64>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SnippetArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "File path relative to project root")]
    pub file_path: String,
    #[schemars(description = "Start line number")]
    pub start_line: Option<i32>,
    #[schemars(description = "End line number")]
    pub end_line: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchCodeArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Text pattern to search for")]
    pub pattern: String,
    #[schemars(description = "Maximum results")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteArgs {
    #[schemars(description = "Project name to delete")]
    pub project: String,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AdrArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Action: list, create, or get")]
    pub action: String,
    #[schemars(description = "ADR title (for create)")]
    pub title: Option<String>,
    #[schemars(description = "ADR content (for create)")]
    pub content: Option<String>,
    #[schemars(description = "ADR ID (for get)")]
    pub id: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct IngestTracesArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Trace data as JSON array")]
    pub traces: Value,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct LinkProjectArgs {
    #[schemars(description = "Source project name")]
    pub project: String,
    #[schemars(description = "Target project name to link/unlink")]
    pub target_project: String,
    #[schemars(description = "Action: link or unlink")]
    pub action: String,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchLinkedArgs {
    #[schemars(description = "Project name to search from (will search its linked projects)")]
    pub project: Option<String>,
    #[schemars(description = "Search query string")]
    pub query: String,
    #[schemars(description = "Optional node label filter (e.g. Function, Class, Method)")]
    pub label: Option<String>,
    #[schemars(description = "Maximum total results")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct FindSymbolArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Symbol name or qualified name to search for")]
    pub query: String,
    #[schemars(description = "Filter by label: Function, Class, Method, Interface, Module, File")]
    pub label: Option<String>,
    #[schemars(description = "Only return exact matches (no fuzzy fallback)")]
    pub exact: Option<bool>,
    #[schemars(description = "Maximum results (default 10)")]
    pub limit: Option<i32>,
    #[schemars(description = "Include results from linked projects, tagged with source_project")]
    pub include_linked: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct GetSymbolDetailsArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Exact qualified name of the symbol")]
    pub qualified_name: Option<String>,
    #[schemars(description = "Symbol name (if qualified_name not known)")]
    pub name: Option<String>,
    #[schemars(description = "Filter by label when resolving by name")]
    pub label: Option<String>,
    #[schemars(description = "Include source code snippet")]
    pub include_snippet: Option<bool>,
    #[schemars(description = "Max snippet lines (default 50 for classes, 150 for functions)")]
    pub snippet_lines: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct FindReferencesArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Exact qualified name of the target symbol")]
    pub qualified_name: Option<String>,
    #[schemars(description = "Symbol name (if qualified_name not known)")]
    pub name: Option<String>,
    #[schemars(description = "Filter by label when resolving by name")]
    pub label: Option<String>,
    #[schemars(description = "Reference type filter: calls, imports, uses, all (default: all)")]
    pub reference_type: Option<String>,
    #[schemars(description = "Maximum references (default 30)")]
    pub limit: Option<i32>,
    #[schemars(description = "Group results by: file (default) or symbol")]
    pub group_by: Option<String>,
    #[schemars(
        description = "Include references from linked projects, tagged with source_project"
    )]
    pub include_linked: Option<bool>,
    #[schemars(description = "Minimum confidence threshold for filtering edges (0.0 to 1.0)")]
    pub min_confidence: Option<f64>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ImpactAnalysisArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Qualified name of the symbol to analyze")]
    pub qualified_name: Option<String>,
    #[schemars(description = "Symbol name (if qualified_name not known)")]
    pub name: Option<String>,
    #[schemars(description = "File path to analyze (aggregates all symbols in file)")]
    pub file_path: Option<String>,
    #[schemars(description = "Max traversal depth (default 3)")]
    pub max_depth: Option<i32>,
    #[schemars(description = "Max total dependents to return (default 50)")]
    pub limit: Option<i32>,
    #[schemars(description = "Include cross-project impact")]
    pub include_linked: Option<bool>,
    #[schemars(description = "Minimum confidence threshold for filtering edges (0.0 to 1.0)")]
    pub min_confidence: Option<f64>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExplainIndexResultArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "File path to diagnose")]
    pub file_path: Option<String>,
    #[schemars(description = "Qualified name of symbol to diagnose")]
    pub qualified_name: Option<String>,
    #[schemars(description = "Symbol name to diagnose")]
    pub name: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct FindPipelinesArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct FindInfrastructureArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Filter by infrastructure type: terraform, kubernetes, docker, helm")]
    pub infra_type: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SampleGraphArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Maximum nodes to return (default 20)")]
    pub limit: Option<i32>,
    #[schemars(
        description = "Sort order: 'degree' (default, by fan_in+fan_out), 'cyclomatic' (by cyclomatic_complexity desc), 'cognitive' (by cognitive_complexity desc), 'hotspot' (by git_commits desc)"
    )]
    pub sort_by: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

// ── Track 5 Agent-First Tool Args ─────────────────────────────────────────

/// Args for `get_project_summary` (Track 5.3)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct GetProjectSummaryArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

/// Args for `get_context_for_task` (Track 5.9)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct GetContextForTaskArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Symbol name to get context for")]
    pub name: Option<String>,
    #[schemars(description = "Exact qualified name of the symbol")]
    pub qualified_name: Option<String>,
    #[schemars(description = "Task type: modify, debug, test, document (default: modify)")]
    pub task: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

/// Args for `get_symbols_batch` (Track 5.12)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct GetSymbolsBatchArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Array of symbol names or qualified names to resolve (max 50)")]
    pub names: Option<Vec<String>>,
    #[schemars(
        description = "Filter: 'class:<ClassName>' for class members, 'file:<path>' for file symbols"
    )]
    pub filter: Option<String>,
    #[schemars(description = "Include edges between the returned symbols (default false)")]
    pub include_internal_edges: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

// ── Milestone 3 Tool Args ─────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WhatIfArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Symbol name or file path to analyze")]
    pub symbol: String,
    #[schemars(description = "Change type: rename, remove, change_signature, move_file")]
    pub change_type: String,
    #[schemars(
        description = "New value (new name for rename, new path for move_file, new signature description)"
    )]
    pub new_value: Option<String>,
    #[schemars(description = "Max traversal depth (default 3)")]
    pub max_depth: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct FindDeadCodeArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Optional folder/module scope filter")]
    pub scope: Option<String>,
    #[schemars(description = "Maximum results (default 50)")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct GetDependencyGraphArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Granularity: file, folder, or package (default: folder)")]
    pub granularity: Option<String>,
    #[schemars(description = "Optional folder/module scope filter")]
    pub scope: Option<String>,
    #[schemars(description = "If true, only return edges involved in cycles")]
    pub include_cycles_only: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct FreshnessCheckArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Optional folder/module scope filter")]
    pub scope: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ClearCacheArgs {
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AskGraphArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Natural language question about the codebase")]
    pub question: String,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct PlanRefactoringArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Target symbol or file to refactor")]
    pub target: String,
    #[schemars(
        description = "Refactoring type: extract_module, split_class, move_function, inline_function, extract_interface"
    )]
    pub refactoring_type: String,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

// ── Milestone 4 Tool Args ─────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ReviewChangesArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "List of changed file paths to review")]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct DetectPatternsArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Only detect patterns (skip antipatterns)")]
    pub patterns_only: Option<bool>,
    #[schemars(description = "Only detect antipatterns (skip patterns)")]
    pub antipatterns_only: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TestCoverageMapArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Optional folder/module scope filter")]
    pub scope: Option<String>,
    #[schemars(description = "Only show untested symbols")]
    pub untested_only: Option<bool>,
    #[schemars(description = "Maximum results (default 50)")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TraceErrorFlowArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Symbol name to trace error flow from")]
    pub symbol: String,
    #[schemars(description = "Max traversal depth (default 5)")]
    pub max_depth: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct GetAPISurfaceArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Filter by file path prefix (module)")]
    pub module_filter: Option<String>,
    #[schemars(description = "Filter by symbol type (Function, Class, Method, etc.)")]
    pub symbol_type: Option<String>,
    #[schemars(description = "Maximum results (default 50)")]
    pub limit: Option<i32>,
    #[schemars(
        description = "Only return symbols that lack documentation (docstring is null or empty)"
    )]
    pub undocumented: Option<bool>,
    #[schemars(
        description = "If true, compare current routes against the most recent snapshot and return added/removed/modified endpoints"
    )]
    pub diff: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

/// Args for `semantic_search` — natural language code search using embeddings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SemanticSearchArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Natural language search query (1-500 characters)")]
    pub query: String,
    #[schemars(description = "Maximum results (default 20)")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

/// Args for `get_graph_diff` — compare two graph snapshots to detect structural drift.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct GetGraphDiffArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(
        description = "Optional snapshot ID to compare from (older). If omitted, uses the second most recent snapshot."
    )]
    pub from_snapshot_id: Option<i64>,
    #[schemars(
        description = "Optional snapshot ID to compare to (newer). If omitted, uses the most recent snapshot."
    )]
    pub to_snapshot_id: Option<i64>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}
