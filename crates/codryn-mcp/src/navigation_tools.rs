use crate::AnalyticsMeta;

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct GetFileOverviewArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "File path relative to project root")]
    pub file_path: String,
    #[schemars(description = "Include symbol list (default true)")]
    pub include_symbols: Option<bool>,
    #[schemars(description = "Include inferred imports (default true)")]
    pub include_imports: Option<bool>,
    #[schemars(description = "Include inferred exports (default true)")]
    pub include_exports: Option<bool>,
    #[schemars(description = "Include graph neighborhood summary (default true)")]
    pub include_neighbors: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct FindEntrypointsArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Optional folder/module scope filter")]
    pub scope: Option<String>,
    #[schemars(
        description = "Entry type filter: http, cli, lambda, bootstrap, route, public_api, any"
    )]
    pub entry_type: Option<String>,
    #[schemars(description = "Maximum results (default 10)")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct SuggestNextReadsArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Qualified name of the origin symbol")]
    pub qualified_name: Option<String>,
    #[schemars(description = "File path of the origin")]
    pub file_path: Option<String>,
    #[schemars(description = "Goal: understand, debug, refactor, trace, test")]
    pub goal: Option<String>,
    #[schemars(description = "Maximum results (default 10)")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct TraceDataFlowArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Source symbol name or qualified name")]
    pub source: Option<String>,
    #[schemars(description = "Target symbol name or qualified name")]
    pub target: Option<String>,
    #[schemars(description = "Source file path")]
    pub file_path: Option<String>,
    #[schemars(description = "Flow type: request, data, render, service, any")]
    pub flow_type: Option<String>,
    #[schemars(description = "Maximum traversal depth (default 5)")]
    pub max_depth: Option<i32>,
    #[schemars(description = "Maximum paths to return (default 10)")]
    pub limit: Option<i32>,
    #[schemars(description = "Include cross-project flows")]
    pub include_linked: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct FindTestsForTargetArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Qualified name of the target symbol")]
    pub qualified_name: Option<String>,
    #[schemars(description = "Symbol name (if qualified_name not known)")]
    pub name: Option<String>,
    #[schemars(description = "File path of the target")]
    pub file_path: Option<String>,
    #[schemars(description = "Maximum results (default 10)")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct SuggestProjectLinksArgs {
    #[schemars(
        description = "Project name (optional, suggests links for all projects if omitted)"
    )]
    pub project: Option<String>,
    #[schemars(description = "Maximum suggestions (default 10)")]
    pub limit: Option<i32>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct FindRoutesArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Optional folder/module scope filter")]
    pub scope: Option<String>,
    #[schemars(description = "Filter by HTTP method (GET, POST, PUT, PATCH, DELETE)")]
    pub method: Option<String>,
    #[schemars(description = "Maximum results (default 20)")]
    pub limit: Option<i32>,
    #[schemars(description = "Include deleted/stale files in results (default false)")]
    pub include_deleted: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct TraceBackendFlowArgs {
    #[schemars(description = "Project name")]
    pub project: Option<String>,
    #[schemars(description = "Route path to trace (e.g. /v1/users/{id})")]
    pub route_path: Option<String>,
    #[schemars(description = "Handler name to trace")]
    pub handler: Option<String>,
    #[schemars(description = "HTTP method filter (GET, POST, PUT, PATCH, DELETE)")]
    pub http_method: Option<String>,
    #[schemars(description = "Maximum traversal depth (default 5)")]
    pub max_depth: Option<i32>,
    #[schemars(description = "Include cross-project flows")]
    pub include_linked: Option<bool>,
    #[serde(default)]
    pub analytics: Option<AnalyticsMeta>,
}
