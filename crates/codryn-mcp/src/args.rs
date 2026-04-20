use serde_json::Value;

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct ProjectArg {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct IndexArgs {
    #[schemars(description = "Absolute path to the repository root")]
    pub path: String,
    #[schemars(description = "Index mode: full or fast")]
    pub mode: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct TraceArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Source function name or qualified name")]
    pub source: String,
    #[schemars(description = "Target function name or qualified name")]
    pub target: String,
    #[schemars(description = "Maximum path depth")]
    pub max_depth: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteArgs {
    #[schemars(description = "Project name to delete")]
    pub project: String,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct IngestTracesArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Trace data as JSON array")]
    pub traces: Value,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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
    #[schemars(description = "Max snippet lines (default 20)")]
    pub snippet_lines: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct FindReferencesArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Exact qualified name of the target symbol")]
    pub qualified_name: Option<String>,
    #[schemars(description = "Symbol name (if qualified_name not known)")]
    pub name: Option<String>,
    #[schemars(description = "Filter by label when resolving by name")]
    pub label: Option<String>,
    #[schemars(description = "Reference type filter: calls, imports, all (default: all)")]
    pub reference_type: Option<String>,
    #[schemars(description = "Maximum references (default 30)")]
    pub limit: Option<i32>,
    #[schemars(description = "Group results by: file (default) or symbol")]
    pub group_by: Option<String>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
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
