mod args;
pub mod auto_index;
pub mod diagnostics;
pub mod health;
mod helpers;
pub mod logging;
mod navigation_tools;
pub mod query_cache;
pub mod rate_limit;
pub mod shutdown;
pub mod tracing_middleware;

pub use args::*;

use codryn_foundation::fqn;
use codryn_pipeline::{IndexMode, Pipeline};
use codryn_services::analytics::{AnalyticsContext, AnalyticsService};
use codryn_services::api_surface;
use codryn_services::architecture::ArchitectureService;
use codryn_services::backend_flow::BackendFlowService;
use codryn_services::dead_code;
use codryn_services::dependency_graph;
use codryn_services::diff_review;
use codryn_services::error_chain;
use codryn_services::flow::FlowAnalysisService;
use codryn_services::navigation::NavigationService;
use codryn_services::nl_to_cypher::NLToCypherService;
use codryn_services::pattern_detection;
use codryn_services::pipeline::PipelineService;
use codryn_services::project_linking::ProjectLinkingService;
use codryn_services::refactoring::{RefactoringService, RefactoringType};
use codryn_services::staleness;
use codryn_services::test_discovery::TestDiscoveryService;
use codryn_services::test_gap;
use codryn_services::what_if;
use codryn_store::Store;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::Parameters;
use rmcp::model::{ProtocolVersion, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use serde_json::{json, Value};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use auto_index::AutoIndexer;
use diagnostics::Diagnostics;
use health::HealthStatus;
use query_cache::QueryCache;
use rate_limit::RateLimiter;

#[derive(Debug, Clone)]
pub struct CodrynServer {
    store_path: PathBuf,
    session_root: Arc<Mutex<Option<String>>>,
    tool_router: ToolRouter<Self>,
    auto_indexer: AutoIndexer,
    diagnostics: Diagnostics,
    start_time: Instant,
    rate_limiter: Arc<RateLimiter>,
    query_cache: QueryCache,
}

impl CodrynServer {
    pub fn new(store_path: &Path) -> Self {
        let config = codryn_foundation::config::AppConfig::load();
        let rate_limiter = match &config.rate_limit {
            Some(rl_config) => RateLimiter::from_config(rl_config),
            None => RateLimiter::new(std::time::Duration::from_secs(60), 10, 500),
        };
        Self {
            store_path: store_path.to_owned(),
            session_root: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
            auto_indexer: AutoIndexer::new(store_path),
            diagnostics: Diagnostics::new(),
            start_time: Instant::now(),
            rate_limiter: Arc::new(rate_limiter),
            query_cache: QueryCache::new(Duration::from_secs(300)),
        }
    }

    async fn get_store(&self) -> anyhow::Result<Store> {
        if self.store_path.to_string_lossy() == ":memory:" {
            Store::open_in_memory()
        } else {
            let sp = self.store_path.clone();
            tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&sp)?;
                Store::open(&sp.join("graph.db"))
            })
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join: {e}"))?
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn analytics_log(
        &self,
        ctx: &AnalyticsContext,
        tool: &str,
        project: &str,
        start: Instant,
        request: &str,
        result: &str,
    ) {
        let duration_ms = start.elapsed().as_millis() as i64;
        AnalyticsService::log_call(
            &self.store_path,
            ctx,
            tool,
            project,
            duration_ms,
            request,
            result,
        );
    }

    fn extract_ctx(meta: &rmcp::model::Meta, fallback: Option<&AnalyticsMeta>) -> AnalyticsContext {
        AnalyticsService::extract(
            meta.0.get("agent_name").and_then(|v| v.as_str()),
            meta.0.get("model_name").and_then(|v| v.as_str()),
            meta.0.get("input_tokens").and_then(|v| v.as_i64()),
            meta.0.get("output_tokens").and_then(|v| v.as_i64()),
            fallback.and_then(|f| f.agent_name.as_deref()),
            fallback.and_then(|f| f.model_name.as_deref()),
            fallback.and_then(|f| f.input_tokens),
            fallback.and_then(|f| f.output_tokens),
        )
    }

    async fn resolve_project(&self, arg: Option<&str>) -> String {
        if let Some(p) = arg {
            if !p.is_empty() {
                return p.to_owned();
            }
        }
        let guard = self.session_root.lock().await;
        guard
            .as_deref()
            .map(fqn::project_name_from_path)
            .unwrap_or_else(|| "default".into())
    }

    /// Trigger a non-blocking auto-reindex check for the given project.
    /// Queries continue serving from the existing index while reindex runs in background.
    fn trigger_auto_reindex(&self, store: &Store, project: &str) {
        if let Ok(projects) = store.list_projects() {
            if let Some(p) = projects.iter().find(|p| p.name == project) {
                self.auto_indexer.check_and_reindex(project, &p.root_path);
            }
        }
    }

    /// Record the duration of a store query in the diagnostics subsystem.
    fn record_query_duration(&self, start: Instant) {
        self.diagnostics.record_query(start.elapsed());
    }

    /// Check rate limits for a tool invocation. Returns an error JSON string
    /// if the session is rate-limited, or None if the request can proceed.
    /// Exempt tools (index_repository, health_check) always return None.
    #[allow(dead_code)]
    fn check_rate_limit(&self, tool_name: &str, session: &str) -> Option<String> {
        if RateLimiter::is_exempt(tool_name) {
            return None;
        }
        if self.rate_limiter.is_limited(session) {
            // Calculate retry_after from the limiter
            let retry_after = match self.rate_limiter.record(session, u64::MAX) {
                Err(e) => e.retry_after_seconds,
                Ok(()) => 0, // shouldn't happen if is_limited was true
            };
            Some(
                json!({
                    "error": "rate_limit_exceeded",
                    "message": "Too many expensive queries. Please wait before retrying.",
                    "retry_after_seconds": retry_after
                })
                .to_string(),
            )
        } else {
            None
        }
    }

    /// Record a completed tool execution for rate limiting purposes.
    /// Should be called after a tool completes with its execution duration.
    #[allow(dead_code)]
    fn record_rate_limit(&self, tool_name: &str, session: &str, duration_ms: u64) {
        if RateLimiter::is_exempt(tool_name) {
            return;
        }
        // Ignore the result — if rate limited, the next call will be rejected
        let _ = self.rate_limiter.record(session, duration_ms);
    }

    /// Read a short code snippet for a node given its file_path and node_id.
    /// Returns up to 10 lines of the symbol's source code, or None if unavailable.
    fn read_node_snippet(
        root: &str,
        file_path: &str,
        node_id: i64,
        store: &Store,
    ) -> Option<String> {
        if file_path.is_empty() || root.is_empty() {
            return None;
        }
        // Get the node's line range from the store
        let node = store.get_node_by_id(node_id).ok()??;
        if node.start_line <= 0 || node.end_line <= 0 {
            return None;
        }
        let full_path = Path::new(root).join(file_path);
        let content = std::fs::read_to_string(&full_path).ok()?;
        let lines: Vec<&str> = content.lines().collect();
        let start = (node.start_line as usize).saturating_sub(1);
        let end = (node.end_line as usize).min(lines.len());
        if start >= end || start >= lines.len() {
            return None;
        }
        // Cap at 10 lines for brevity
        let cap_end = end.min(start + 10);
        Some(lines[start..cap_end].join("\n"))
    }

    /// Thin public wrapper around `index_repository` for integration testing.
    #[doc(hidden)]
    pub async fn index_repository_test(&self, args: crate::IndexArgs) -> String {
        use rmcp::handler::server::tool::Parameters;
        self.index_repository(Parameters(args), rmcp::model::Meta::default())
            .await
    }

    /// Thin public wrapper around `find_symbol` for integration testing.
    #[doc(hidden)]
    pub async fn find_symbol_test(&self, args: crate::FindSymbolArgs) -> String {
        use rmcp::handler::server::tool::Parameters;
        self.find_symbol(Parameters(args), rmcp::model::Meta::default())
            .await
    }

    /// Thin public wrapper around `get_symbol_details` for integration testing.
    #[doc(hidden)]
    pub async fn get_symbol_details_test(&self, args: crate::GetSymbolDetailsArgs) -> String {
        use rmcp::handler::server::tool::Parameters;
        self.get_symbol_details(Parameters(args), rmcp::model::Meta::default())
            .await
    }

    /// Thin public wrapper around `find_references` for integration testing.
    #[doc(hidden)]
    pub async fn find_references_test(&self, args: crate::FindReferencesArgs) -> String {
        use rmcp::handler::server::tool::Parameters;
        self.find_references(Parameters(args), rmcp::model::Meta::default())
            .await
    }

    /// Thin public wrapper around `search_graph` for integration testing.
    #[doc(hidden)]
    pub async fn search_graph_test(&self, args: crate::SearchArgs) -> String {
        use rmcp::handler::server::tool::Parameters;
        self.search_graph(Parameters(args), rmcp::model::Meta::default())
            .await
    }

    /// Thin public wrapper around `get_graph_diff` for integration testing.
    #[doc(hidden)]
    pub async fn get_graph_diff_test(&self, args: crate::GetGraphDiffArgs) -> String {
        use rmcp::handler::server::tool::Parameters;
        self.get_graph_diff(Parameters(args), rmcp::model::Meta::default())
            .await
    }
}

#[tool_router]
impl CodrynServer {
    #[tool(description = "List all indexed projects with their metadata")]
    async fn list_projects(&self, meta: rmcp::model::Meta) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, None);
        let req = String::new();
        let result = match self.get_store().await.and_then(|s| s.list_projects()) {
            Ok(projects) => {
                serde_json::to_string(&json!({ "projects": projects })).unwrap_or_default()
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "list_projects", "", start, &req, &result);
        result
    }

    #[tool(description = "Get the graph schema (node labels, edge types, counts) for a project")]
    async fn get_graph_schema(
        &self,
        Parameters(args): Parameters<ProjectArg>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(s) => {
                self.trigger_auto_reindex(&s, &project);
                match s.get_graph_schema(&project) {
                    Ok(schema) => serde_json::to_string(&schema).unwrap_or_default(),
                    Err(e) => json!({ "error": e.to_string() }).to_string(),
                }
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "get_graph_schema", &project, start, &req, &result);
        result
    }

    #[tool(description = "Index a repository to build the knowledge graph")]
    async fn index_repository(
        &self,
        Parameters(args): Parameters<IndexArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let clear_cache = args.clear_cache.unwrap_or(false);
        let mode = match args.mode.as_deref() {
            Some("fast") if !clear_cache => IndexMode::Fast,
            _ => IndexMode::Full,
        };
        let repo = PathBuf::from(&args.path);

        // Validate that the repository path exists and is a directory
        if !repo.exists() {
            let r = json!({ "error": format!("repository path does not exist: {}", args.path) })
                .to_string();
            self.analytics_log(&ctx, "index_repository", "", start, &req, &r);
            return r;
        }
        if !repo.is_dir() {
            let r =
                json!({ "error": format!("repository path is not a directory: {}", args.path) })
                    .to_string();
            self.analytics_log(&ctx, "index_repository", "", start, &req, &r);
            return r;
        }

        let store_path = self.store_path.clone();
        let project_name = Pipeline::new(&repo, &store_path, mode).project_name();

        // If clear_cache is requested, wipe all existing data for this project first.
        // This forces a full rebuild from scratch regardless of stored hashes.
        if clear_cache {
            match self.get_store().await {
                Err(e) => {
                    let r = json!({ "error": format!("failed to clear cache: {}", e) }).to_string();
                    self.analytics_log(&ctx, "index_repository", &project_name, start, &req, &r);
                    return r;
                }
                Ok(store) => {
                    if let Err(e) = store.delete_project_data(&project_name) {
                        let r =
                            json!({ "error": format!("failed to clear cache: {}", e) }).to_string();
                        self.analytics_log(
                            &ctx,
                            "index_repository",
                            &project_name,
                            start,
                            &req,
                            &r,
                        );
                        return r;
                    }
                    tracing::info!(project = %project_name, "index_repository: cache cleared, starting full reindex");
                }
            }
        }

        let run_result =
            tokio::task::spawn_blocking(move || Pipeline::new(&repo, &store_path, mode).run())
                .await;

        let result = match run_result {
            Ok(Ok(())) => {
                let mut guard = self.session_root.lock().await;
                *guard = Some(args.path.clone());
                // Auto-link projects with high confidence
                let mut auto_linked = Vec::new();
                if let Ok(store) = self.get_store().await {
                    if let Ok(suggestions) =
                        ProjectLinkingService::suggest_links(&store, Some(&project_name), 10)
                    {
                        for s in &suggestions.suggestions {
                            if s.score >= 0.5
                                && store.link_projects(&s.project, &s.target_project).is_ok()
                            {
                                auto_linked.push(json!({"project": s.project, "target": s.target_project, "reason": s.reason}));
                            }
                        }
                    }
                }
                let mut resp = json!({ "status": "ok", "project": project_name });
                if !auto_linked.is_empty() {
                    resp["auto_linked"] = json!(auto_linked);
                }
                // Add memory usage reporting (Requirement 26.1, 26.2, 26.3)
                if let Some(peak_mb) = codryn_pipeline::memory::peak_rss_mb() {
                    resp["memory_usage_mb"] = json!(peak_mb);
                    if let Some(warning) = codryn_pipeline::memory::memory_warning(None) {
                        resp["memory_warning"] = json!(warning);
                    }
                }
                resp.to_string()
            }
            Ok(Err(e)) => json!({ "error": e.to_string() }).to_string(),
            Err(e) => json!({ "error": format!("task panicked: {}", e) }).to_string(),
        };
        self.analytics_log(
            &ctx,
            "index_repository",
            &project_name,
            start,
            &req,
            &result,
        );
        result
    }

    #[tool(description = "Search the knowledge graph for nodes matching a query")]
    async fn search_graph(
        &self,
        Parameters(args): Parameters<SearchArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(20);
        let result = match self.get_store().await {
            Ok(s) => {
                self.trigger_auto_reindex(&s, &project);
                let query_start = Instant::now();
                let nodes = s
                    .search_nodes_broad(&project, &args.query, None, limit)
                    .unwrap_or_default();
                self.record_query_duration(query_start);
                let projects = s.list_projects().unwrap_or_default();
                let root = projects
                    .iter()
                    .find(|p| p.name == project)
                    .map(|p| p.root_path.as_str())
                    .unwrap_or("");
                let items: Vec<Value> = nodes
                    .iter()
                    .map(|n| {
                        let exists =
                            !n.file_path.is_empty() && Path::new(root).join(&n.file_path).exists();
                        json!({
                            "id": n.id, "name": n.name, "qualified_name": n.qualified_name,
                            "label": n.label, "file_path": n.file_path,
                            "start_line": n.start_line, "end_line": n.end_line,
                            "exists": exists,
                        })
                    })
                    .collect();
                let count = items.len();
                let has_more = count as i32 == limit;
                json!({ "nodes": items, "count": count, "has_more": has_more }).to_string()
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "search_graph", &project, start, &req, &result);
        result
    }

    #[tool(description = "Execute a Cypher query against the knowledge graph")]
    async fn query_graph(
        &self,
        Parameters(args): Parameters<QueryArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "query_graph", &project, start, &req, &r);
                return r;
            }
        };
        let query_start = Instant::now();
        let local = match codryn_cypher::execute(&store, &project, &args.query) {
            Ok(r) => {
                self.record_query_duration(query_start);
                r
            }
            Err(e) => {
                self.record_query_duration(query_start);
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "query_graph", &project, start, &req, &r);
                return r;
            }
        };

        if !args.include_linked.unwrap_or(false) {
            let r = local.to_string();
            self.analytics_log(&ctx, "query_graph", &project, start, &req, &r);
            return r;
        }

        let links = store.get_linked_projects(&project).unwrap_or_default();
        if links.is_empty() {
            let r = local.to_string();
            self.analytics_log(&ctx, "query_graph", &project, start, &req, &r);
            return r;
        }

        let mut all_results = vec![json!({ "project": project, "result": local })];
        for link in &links {
            if let Ok(r) = codryn_cypher::execute(&store, &link.target_project, &args.query) {
                all_results.push(json!({ "project": link.target_project, "result": r }))
            }
        }
        let result = json!({ "results": all_results }).to_string();
        self.analytics_log(&ctx, "query_graph", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Search the knowledge graph using natural language semantic similarity. Falls back to text search when embeddings are unavailable."
    )]
    async fn semantic_search(
        &self,
        Parameters(args): Parameters<SemanticSearchArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;

        // Validate query length (1-500 characters)
        if let Err(e) = codryn_services::semantic_search::validate_query(&args.query) {
            let r = json!({ "error": e.to_string() }).to_string();
            self.analytics_log(&ctx, "semantic_search", &project, start, &req, &r);
            return r;
        }

        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "semantic_search", &project, start, &req, &r);
                return r;
            }
        };

        self.trigger_auto_reindex(&store, &project);

        // Try semantic search first; fall back to search_graph if unavailable
        let limit = args.limit.unwrap_or(20).min(20) as usize;
        let query = args.query.clone();

        // Attempt to use the semantic search service
        let semantic_result = {
            let mut service = codryn_services::semantic_search::SemanticSearchService::new().ok();

            if let Some(ref mut svc) = service {
                svc.search(&store, &project, &query).ok()
            } else {
                None
            }
        };

        let result = if let Some(results) = semantic_result {
            // Semantic search succeeded — build response with snippets
            let projects = store.list_projects().unwrap_or_default();
            let root = projects
                .iter()
                .find(|p| p.name == project)
                .map(|p| p.root_path.clone())
                .unwrap_or_default();

            let items: Vec<Value> = results
                .into_iter()
                .take(limit)
                .map(|r| {
                    let snippet = Self::read_node_snippet(&root, &r.file_path, r.node_id, &store);
                    let mut item = json!({
                        "name": r.name,
                        "qualified_name": r.qualified_name,
                        "file_path": r.file_path,
                        "similarity": (r.similarity * 1000.0).round() / 1000.0,
                        "label": r.label,
                    });
                    if let Some(s) = snippet {
                        item["snippet"] = json!(s);
                    }
                    item
                })
                .collect();

            let count = items.len();
            json!({ "results": items, "count": count }).to_string()
        } else {
            // Fall back to search_graph text matching
            let query_start = Instant::now();
            let nodes = store
                .search_nodes_broad(&project, &query, None, limit as i32)
                .unwrap_or_default();
            self.record_query_duration(query_start);

            let projects = store.list_projects().unwrap_or_default();
            let root = projects
                .iter()
                .find(|p| p.name == project)
                .map(|p| p.root_path.clone())
                .unwrap_or_default();

            let items: Vec<Value> = nodes
                .iter()
                .map(|n| {
                    let snippet = Self::read_node_snippet(&root, &n.file_path, n.id, &store);
                    let mut item = json!({
                        "name": n.name,
                        "qualified_name": n.qualified_name,
                        "file_path": n.file_path,
                        "similarity": null,
                        "label": n.label,
                    });
                    if let Some(s) = snippet {
                        item["snippet"] = json!(s);
                    }
                    item
                })
                .collect();

            let count = items.len();
            json!({
                "results": items,
                "count": count,
                "fallback": true,
                "warning": "Semantic search unavailable for this project. Falling back to text-based search."
            })
            .to_string()
        };

        self.analytics_log(&ctx, "semantic_search", &project, start, &req, &result);
        result
    }

    #[tool(description = "Get the index status for a project")]
    async fn index_status(
        &self,
        Parameters(args): Parameters<ProjectArg>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => match store.get_graph_schema(&project) {
                Ok(schema) => {
                    let mut warnings: Vec<String> = Vec::new();
                    if schema.total_nodes > 10 && schema.total_edges == 0 {
                        warnings.push("No edges found despite having nodes. Call resolution may have failed. Try re-indexing.".into());
                    }
                    if schema.total_nodes > 0
                        && !schema
                            .edge_types
                            .iter()
                            .any(|e| e.edge_type == "CALLS" || e.edge_type == "USES")
                    {
                        warnings.push("No CALLS/USES edges. trace_call_path and impact_analysis will return empty.".into());
                    }
                    if schema.total_nodes > 0
                        && !schema.edge_types.iter().any(|e| e.edge_type == "IMPORTS")
                    {
                        warnings.push("No IMPORTS edges. get_symbol_details imports/imported_by will be empty.".into());
                    }
                    if schema.node_labels.iter().any(|n| n.label == "Route")
                        && !schema
                            .edge_types
                            .iter()
                            .any(|e| e.edge_type == "HANDLES_ROUTE")
                    {
                        warnings.push(
                            "Routes exist but no HANDLES_ROUTE edges. find_routes may be incomplete."
                                .into(),
                        );
                    }
                    // Check for Vue files without Selector nodes (Vue adapter didn't run)
                    if schema.total_nodes > 0 {
                        let has_vue_files = store
                            .list_files(&project)
                            .unwrap_or_default()
                            .iter()
                            .any(|f| f.ends_with(".vue"));
                        if has_vue_files
                            && !schema.node_labels.iter().any(|n| n.label == "Selector")
                        {
                            warnings.push(
                                "Project has .vue files but no Selector nodes. Vue adapter may not have run — try a full re-index."
                                    .into(),
                            );
                        }
                    }
                    let mut result = json!({
                        "project": project,
                        "indexed": schema.total_nodes > 0,
                        "total_nodes": schema.total_nodes,
                        "total_edges": schema.total_edges,
                    });
                    if !warnings.is_empty() {
                        result["warnings"] = json!(warnings);
                    }
                    result.to_string()
                }
                Err(e) => json!({ "error": e.to_string() }).to_string(),
            },
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "index_status", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Trace a call path between two functions. Supports min_confidence to filter low-confidence edges. Results include caller/callee file paths."
    )]
    async fn trace_call_path(
        &self,
        Parameters(args): Parameters<TraceArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let max_depth = args.max_depth.unwrap_or(5);
        let min_confidence = args.min_confidence;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "trace_call_path", &project, start, &req, &r);
                return r;
            }
        };

        let target = if args.target.is_empty() {
            None
        } else {
            Some(args.target.as_str())
        };
        let steps = match store.trace_calls_with_confidence(
            &project,
            &args.source,
            target,
            max_depth,
            min_confidence,
        ) {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "trace_call_path", &project, start, &req, &r);
                return r;
            }
        };

        if !steps.is_empty() {
            let path: Vec<_> = steps
                .iter()
                .map(|(src, tgt, src_file, tgt_file)| {
                    json!({
                        "caller": src, "callee": tgt,
                        "caller_file": src_file, "callee_file": tgt_file,
                    })
                })
                .collect();
            let r = json!({ "path": path, "steps": path.len() }).to_string();
            self.analytics_log(&ctx, "trace_call_path", &project, start, &req, &r);
            return r;
        }

        // Fall back to cross-project search if nothing found locally
        let links = store.get_linked_projects(&project).unwrap_or_default();
        let mut cross_results = Vec::new();
        for link in &links {
            let nodes = store
                .search_nodes_filtered(&link.target_project, &args.target, Some("Function"), 5)
                .unwrap_or_default();
            for n in nodes {
                if n.name == args.target || n.qualified_name.ends_with(&args.target) {
                    cross_results.push(json!({
                        "source_project": project,
                        "source_function": args.source,
                        "target_project": link.target_project,
                        "target_function": n.name,
                        "target_file": n.file_path,
                        "cross_project": true,
                    }));
                }
            }
        }

        let result = if cross_results.is_empty() {
            json!({ "path": [], "message": format!("No call path found from '{}'", args.source) })
                .to_string()
        } else {
            json!({ "path": [], "cross_project_matches": cross_results }).to_string()
        };
        self.analytics_log(&ctx, "trace_call_path", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Get the high-level architecture (modules, packages, folders) of a project"
    )]
    async fn get_architecture(
        &self,
        Parameters(args): Parameters<ProjectArg>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => match ArchitectureService::get_architecture(&store, &project) {
                Ok(arch) => serde_json::to_string(&arch)
                    .unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()),
                Err(e) => json!({"error": e.to_string()}).to_string(),
            },
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "get_architecture", &project, start, &req, &result);
        result
    }

    #[tool(description = "Get a code snippet from a file in the indexed project")]
    async fn get_code_snippet(
        &self,
        Parameters(args): Parameters<SnippetArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let timer = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "get_code_snippet", &project, timer, &req, &r);
                return r;
            }
        };
        let projects = store.list_projects().unwrap_or_default();
        let root = projects
            .iter()
            .find(|p| p.name == project)
            .map(|p| p.root_path.clone())
            .unwrap_or_default();

        let full_path = Path::new(&root).join(&args.file_path);
        let file_path_arg = args.file_path.clone();
        let start_line_arg = args.start_line;
        let end_line_arg = args.end_line;
        let result = if full_path.is_dir() {
            // Directory mode: list files and their symbols
            let dir_prefix = if args.file_path.ends_with('/') {
                args.file_path.clone()
            } else {
                format!("{}/", args.file_path)
            };
            let symbols = store
                .list_symbols_in_directory(&project, &dir_prefix, 200)
                .unwrap_or_default();
            let mut files: std::collections::BTreeMap<String, Vec<Value>> =
                std::collections::BTreeMap::new();
            for n in &symbols {
                files.entry(n.file_path.clone()).or_default().push(json!({
                    "name": n.name, "label": n.label,
                    "start_line": n.start_line, "end_line": n.end_line,
                }));
            }
            let file_list: Vec<Value> = files
                .into_iter()
                .map(|(fp, syms)| json!({"file_path": fp, "symbols": syms}))
                .collect();
            json!({
                "directory": args.file_path,
                "files": file_list,
                "total_symbols": symbols.len(),
            })
            .to_string()
        } else {
            let full_path_clone = full_path.clone();
            match tokio::task::spawn_blocking(move || std::fs::read_to_string(&full_path_clone))
                .await
            {
                Ok(Ok(content)) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let start = start_line_arg.unwrap_or(1).max(1) as usize - 1;
                    let end = end_line_arg
                        .map(|e| e as usize)
                        .unwrap_or(lines.len())
                        .min(lines.len());
                    if start >= end {
                        return json!({
                            "error": format!(
                                "invalid line range: start_line ({}) must be less than end_line ({})",
                                start + 1,
                                end
                            )
                        })
                        .to_string();
                    }
                    if start >= lines.len() {
                        return json!({
                            "error": format!(
                                "start_line ({}) exceeds file length ({} lines)",
                                start + 1,
                                lines.len()
                            )
                        })
                        .to_string();
                    }
                    let snippet: Vec<&str> = lines[start..end].to_vec();
                    json!({
                        "file_path": file_path_arg,
                        "start_line": start + 1,
                        "end_line": end,
                        "content": snippet.join("\n"),
                    })
                    .to_string()
                }
                Ok(Err(e)) => json!({ "error": e.to_string() }).to_string(),
                Err(e) => json!({ "error": format!("task panicked: {}", e) }).to_string(),
            }
        };
        self.analytics_log(&ctx, "get_code_snippet", &project, timer, &req, &result);
        result
    }

    #[tool(description = "Search for text patterns in source files of the indexed project")]
    async fn search_code(
        &self,
        Parameters(args): Parameters<SearchCodeArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(20) as usize;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "search_code", &project, start, &req, &r);
                return r;
            }
        };
        let projects = store.list_projects().unwrap_or_default();
        let root = projects
            .iter()
            .find(|p| p.name == project)
            .map(|p| p.root_path.clone())
            .unwrap_or_default();

        let files = store.list_files(&project).unwrap_or_default();
        let pattern = args.pattern.clone();
        let root_owned = root.clone();
        let result = match tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            for file_path in &files {
                if results.len() >= limit {
                    break;
                }
                let full = Path::new(&root_owned).join(file_path);
                if let Ok(content) = std::fs::read_to_string(&full) {
                    for (i, line) in content.lines().enumerate() {
                        if results.len() >= limit {
                            break;
                        }
                        if line.contains(&pattern) {
                            results.push(json!({
                                "file": file_path,
                                "line": i + 1,
                                "content": line.trim(),
                            }));
                        }
                    }
                }
            }
            results
        })
        .await
        {
            Ok(results) => {
                let count = results.len();
                let has_more = count >= limit;
                json!({ "matches": results, "count": count, "has_more": has_more }).to_string()
            }
            Err(e) => json!({ "error": format!("task panicked: {}", e) }).to_string(),
        };
        self.analytics_log(&ctx, "search_code", &project, start, &req, &result);
        result
    }

    #[tool(description = "Detect uncommitted changes in the repository")]
    async fn detect_changes(
        &self,
        Parameters(args): Parameters<ProjectArg>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => return json!({ "error": e.to_string() }).to_string(),
        };
        let projects = store.list_projects().unwrap_or_default();
        let root = projects
            .iter()
            .find(|p| p.name == project)
            .map(|p| p.root_path.clone())
            .unwrap_or_default();

        let root_clone = root.clone();
        let result = match tokio::task::spawn_blocking(move || {
            std::process::Command::new("git")
                .args(["diff", "--name-only", "HEAD"])
                .current_dir(&root_clone)
                .output()
        })
        .await
        {
            Ok(Ok(output)) => {
                let files: Vec<&str> = std::str::from_utf8(&output.stdout)
                    .unwrap_or("")
                    .lines()
                    .filter(|l| !l.is_empty())
                    .collect();
                json!({ "changed_files": files, "count": files.len() }).to_string()
            }
            Ok(Err(e)) => json!({ "error": e.to_string() }).to_string(),
            Err(e) => json!({ "error": format!("task panicked: {}", e) }).to_string(),
        };
        self.analytics_log(&ctx, "detect_changes", &project, start, &req, &result);
        result
    }

    #[tool(description = "Delete an indexed project and all its data")]
    async fn delete_project(
        &self,
        Parameters(args): Parameters<DeleteArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let result = match self
            .get_store()
            .await
            .and_then(|s| s.delete_project(&args.project))
        {
            Ok(()) => json!({ "status": "deleted", "project": args.project }).to_string(),
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "delete_project", &args.project, start, &req, &result);
        result
    }

    #[tool(description = "Manage Architecture Decision Records (ADRs)")]
    async fn manage_adr(
        &self,
        Parameters(args): Parameters<AdrArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "manage_adr", &project, start, &req, &r);
                return r;
            }
        };
        let result = match args.action.as_str() {
            "list" => match store.list_adrs(&project) {
                Ok(adrs) => json!({ "adrs": adrs, "count": adrs.len() }).to_string(),
                Err(e) => json!({ "error": e.to_string() }).to_string(),
            },
            "create" => {
                let title = args.title.as_deref().unwrap_or("Untitled");
                let content = args.content.as_deref().unwrap_or("");
                let id = args
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("ADR-{:03}", chrono::Utc::now().timestamp() % 1000));
                match store.create_adr(&project, &id, title, content) {
                    Ok(()) => json!({ "status": "created", "id": id, "title": title }).to_string(),
                    Err(e) => json!({ "error": e.to_string() }).to_string(),
                }
            }
            "get" => {
                let id = args.id.as_deref().unwrap_or("");
                match store.get_adr(&project, id) {
                    Ok(Some(adr)) => serde_json::to_string(&adr).unwrap_or_default(),
                    Ok(None) => json!({ "error": "ADR not found" }).to_string(),
                    Err(e) => json!({ "error": e.to_string() }).to_string(),
                }
            }
            _ => json!({ "error": "action must be 'list', 'create', or 'get'" }).to_string(),
        };
        self.analytics_log(&ctx, "manage_adr", &project, start, &req, &result);
        result
    }

    #[tool(description = "Ingest runtime traces to enrich the knowledge graph")]
    async fn ingest_traces(
        &self,
        Parameters(args): Parameters<IngestTracesArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "ingest_traces", &project, start, &req, &r);
                return r;
            }
        };
        let traces = match args.traces.as_array() {
            Some(t) => t,
            None => return json!({ "error": "traces must be an array" }).to_string(),
        };
        let mut ingested = 0usize;
        for trace in traces {
            let src = trace["source"].as_str().unwrap_or("");
            let tgt = trace["target"].as_str().unwrap_or("");
            let edge_type = trace["type"].as_str().unwrap_or("CALLS");
            if !src.is_empty()
                && !tgt.is_empty()
                && store.ingest_trace(&project, src, tgt, edge_type).is_ok()
            {
                ingested += 1;
            }
        }
        let result =
            json!({ "status": "ingested", "count": ingested, "total": traces.len() }).to_string();
        self.analytics_log(&ctx, "ingest_traces", &project, start, &req, &result);
        result
    }

    #[tool(description = "Link or unlink two projects for cross-project querying")]
    async fn link_project(
        &self,
        Parameters(args): Parameters<LinkProjectArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "link_project", "", start, &req, &r);
                return r;
            }
        };
        // Validate both projects exist
        let projects = store.list_projects().unwrap_or_default();
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        if !names.contains(&args.project.as_str()) {
            return json!({ "error": format!("Project '{}' not found", args.project) }).to_string();
        }
        if !names.contains(&args.target_project.as_str()) {
            return json!({ "error": format!("Project '{}' not found", args.target_project) })
                .to_string();
        }
        let result = match args.action.as_str() {
            "link" => match store.link_projects(&args.project, &args.target_project) {
                Ok(()) => json!({ "status": "linked", "project": args.project, "target_project": args.target_project }).to_string(),
                Err(e) => json!({ "error": e.to_string() }).to_string(),
            },
            "unlink" => match store.unlink_projects(&args.project, &args.target_project) {
                Ok(()) => json!({ "status": "unlinked", "project": args.project, "target_project": args.target_project }).to_string(),
                Err(e) => json!({ "error": e.to_string() }).to_string(),
            },
            _ => json!({ "error": "action must be 'link' or 'unlink'" }).to_string(),
        };
        self.analytics_log(&ctx, "link_project", &args.project, start, &req, &result);
        result
    }

    #[tool(description = "List all projects linked to a given project")]
    async fn list_project_links(
        &self,
        Parameters(args): Parameters<ProjectArg>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self
            .get_store()
            .await
            .and_then(|s| s.get_linked_projects(&project))
        {
            Ok(links) => {
                let targets: Vec<&str> = links.iter().map(|l| l.target_project.as_str()).collect();
                json!({ "project": project, "linked_projects": targets, "count": links.len() })
                    .to_string()
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "list_project_links", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Search across all linked projects' knowledge graphs. Results are tagged with source_project."
    )]
    async fn search_linked_projects(
        &self,
        Parameters(args): Parameters<SearchLinkedArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(20);
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => return json!({ "error": e.to_string() }).to_string(),
        };
        let links = store.get_linked_projects(&project).unwrap_or_default();
        if links.is_empty() {
            return json!({ "error": format!("No linked projects for '{}'", project) }).to_string();
        }
        let mut results = Vec::new();
        for link in &links {
            let remaining = limit - results.len() as i32;
            if remaining <= 0 {
                break;
            }
            let nodes = store
                .search_nodes_broad(
                    &link.target_project,
                    &args.query,
                    args.label.as_deref(),
                    remaining,
                )
                .unwrap_or_default();
            let linked_projects = store.list_projects().unwrap_or_default();
            let linked_root = linked_projects
                .iter()
                .find(|p| p.name == link.target_project)
                .map(|p| p.root_path.as_str())
                .unwrap_or("");
            for n in nodes {
                let exists =
                    !n.file_path.is_empty() && Path::new(linked_root).join(&n.file_path).exists();
                results.push(json!({
                    "source_project": link.target_project,
                    "name": n.name,
                    "qualified_name": n.qualified_name,
                    "label": n.label,
                    "file_path": n.file_path,
                    "start_line": n.start_line,
                    "end_line": n.end_line,
                    "exists": exists,
                }));
            }
        }
        let count = results.len();
        let has_more = count as i32 >= limit;
        let result = json!({ "from_project": project, "results": results, "count": count, "has_more": has_more })
            .to_string();
        self.analytics_log(
            &ctx,
            "search_linked_projects",
            &project,
            start,
            &req,
            &result,
        );
        result
    }

    // ── New Agent-Optimized Tools ─────────────────────────

    #[tool(
        description = "Find a symbol by name or qualified name with ranked results. Faster and more precise than search_graph for symbol lookup."
    )]
    async fn find_symbol(
        &self,
        Parameters(args): Parameters<FindSymbolArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(10);
        let exact = args.exact.unwrap_or(false);
        let include_linked = args.include_linked.unwrap_or(false);
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "find_symbol", &project, start, &req, &r);
                return r;
            }
        };
        self.trigger_auto_reindex(&store, &project);
        let query_start = Instant::now();
        let result = match store.find_symbol_ranked(
            &project,
            &args.query,
            args.label.as_deref(),
            exact,
            limit,
        ) {
            Ok(matches) => {
                self.record_query_duration(query_start);
                let projects = store.list_projects().unwrap_or_default();
                let root = projects
                    .iter()
                    .find(|p| p.name == project)
                    .map(|p| p.root_path.as_str())
                    .unwrap_or("");
                let mut items: Vec<Value> = matches.iter().map(|(n, mt, sc)| {
                    let exists = !n.file_path.is_empty() && Path::new(root).join(&n.file_path).exists();
                    json!({
                        "name": n.name, "qualified_name": n.qualified_name, "label": n.label,
                        "file_path": n.file_path, "start_line": n.start_line, "end_line": n.end_line,
                        "match_type": mt, "score": sc, "exists": exists,
                    })
                }).collect();

                // Cross-project search
                if include_linked {
                    let links = store.get_linked_projects(&project).unwrap_or_default();
                    for link in &links {
                        let remaining = limit - items.len() as i32;
                        if remaining <= 0 {
                            break;
                        }
                        if let Ok(linked_matches) = store.find_symbol_ranked(
                            &link.target_project,
                            &args.query,
                            args.label.as_deref(),
                            exact,
                            remaining,
                        ) {
                            let linked_root = projects
                                .iter()
                                .find(|p| p.name == link.target_project)
                                .map(|p| p.root_path.as_str())
                                .unwrap_or("");
                            for (n, mt, sc) in &linked_matches {
                                let exists = !n.file_path.is_empty()
                                    && Path::new(linked_root).join(&n.file_path).exists();
                                items.push(json!({
                                    "name": n.name, "qualified_name": n.qualified_name, "label": n.label,
                                    "file_path": n.file_path, "start_line": n.start_line, "end_line": n.end_line,
                                    "match_type": mt, "score": sc, "exists": exists,
                                    "source_project": link.target_project,
                                }));
                            }
                        }
                    }
                }

                let count = items.len();
                let has_more = count as i32 >= limit;
                json!({"project": project, "matches": items, "count": count, "has_more": has_more})
                    .to_string()
            }
            Err(e) => {
                self.record_query_duration(query_start);
                json!({"error": e.to_string()}).to_string()
            }
        };
        self.analytics_log(&ctx, "find_symbol", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Get detailed context for a symbol: metadata, callers, callees, imports, inheritance. One call gives full local context."
    )]
    async fn get_symbol_details(
        &self,
        Parameters(args): Parameters<GetSymbolDetailsArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "get_symbol_details", &project, start, &req, &r);
                return r;
            }
        };
        self.trigger_auto_reindex(&store, &project);

        // Resolve symbol
        let (node, alternatives) = if let Some(qn) = &args.qualified_name {
            match store.find_node_by_qn(&project, qn) {
                Ok(Some(n)) => (n, vec![]),
                Ok(None) => {
                    let r = json!({"error": format!("Symbol not found: {}", qn)}).to_string();
                    self.analytics_log(&ctx, "get_symbol_details", &project, start, &req, &r);
                    return r;
                }
                Err(e) => {
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "get_symbol_details", &project, start, &req, &r);
                    return r;
                }
            }
        } else if let Some(name) = &args.name {
            match store.find_symbol_ranked(&project, name, args.label.as_deref(), false, 5) {
                Ok(matches) if !matches.is_empty() => {
                    let best = matches[0].0.clone();
                    let alts: Vec<Value> = matches.iter().skip(1).map(|(n, _, _)| json!({
                        "name": n.name, "qualified_name": n.qualified_name, "label": n.label,
                        "file_path": n.file_path,
                    })).collect();
                    (best, alts)
                }
                Ok(_) => {
                    let r = json!({"error": format!("Symbol not found: {}", name)}).to_string();
                    self.analytics_log(&ctx, "get_symbol_details", &project, start, &req, &r);
                    return r;
                }
                Err(e) => {
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "get_symbol_details", &project, start, &req, &r);
                    return r;
                }
            }
        } else {
            let r = json!({"error": "Provide qualified_name or name"}).to_string();
            self.analytics_log(&ctx, "get_symbol_details", &project, start, &req, &r);
            return r;
        };

        let neighbor_limit = 10i32;
        let call_types = &["CALLS", "ASYNC_CALLS", "HTTP_CALLS"];
        let import_types = &["IMPORTS"];
        let inherit_types = &["INHERITS"];
        let impl_types = &["IMPLEMENTS"];

        let to_json = |items: &[(String, String, String, String, i32, String)]| -> Vec<Value> {
            items.iter().map(|(name, qn, label, fp, sl, _)| json!({
                "name": name, "qualified_name": qn, "label": label, "file_path": fp, "line": sl,
            })).collect()
        };

        let callers = store
            .node_neighbors_detailed(node.id, "in", Some(call_types), neighbor_limit)
            .unwrap_or_default();
        let callees = store
            .node_neighbors_detailed(node.id, "out", Some(call_types), neighbor_limit)
            .unwrap_or_default();
        let imports = store
            .node_neighbors_detailed(node.id, "out", Some(import_types), neighbor_limit)
            .unwrap_or_default();
        let imported_by = store
            .node_neighbors_detailed(node.id, "in", Some(import_types), neighbor_limit)
            .unwrap_or_default();
        let inherits = store
            .node_neighbors_detailed(node.id, "out", Some(inherit_types), neighbor_limit)
            .unwrap_or_default();
        let implements = store
            .node_neighbors_detailed(node.id, "out", Some(impl_types), neighbor_limit)
            .unwrap_or_default();
        let renders_types = &["RENDERS"];
        let maps_to_types = &["MAPS_TO"];
        let renders = store
            .node_neighbors_detailed(node.id, "out", Some(renders_types), neighbor_limit)
            .unwrap_or_default();
        let maps_to = store
            .node_neighbors_detailed(node.id, "out", Some(maps_to_types), neighbor_limit)
            .unwrap_or_default();

        let mut resp = json!({
            "project": project,
            "symbol": {
                "name": node.name, "qualified_name": node.qualified_name, "label": node.label,
                "file_path": node.file_path, "start_line": node.start_line, "end_line": node.end_line,
            },
            "callers": to_json(&callers),
            "callees": to_json(&callees),
            "imports": to_json(&imports),
            "imported_by": to_json(&imported_by),
            "relationships": {
                "inherits": to_json(&inherits), "implements": to_json(&implements),
                "renders": to_json(&renders), "maps_to": to_json(&maps_to),
            },
            "alternatives": alternatives,
        });

        // Inject complexity metrics from properties_json if present
        if let Some(ref pj) = node.properties_json {
            if let Ok(pv) = serde_json::from_str::<Value>(pj) {
                if let Some(cyc) = pv.get("cyclomatic_complexity") {
                    resp["symbol"]["cyclomatic_complexity"] = cyc.clone();
                }
                if let Some(cog) = pv.get("cognitive_complexity") {
                    resp["symbol"]["cognitive_complexity"] = cog.clone();
                }
            }
        }

        // For Class/Interface: add members summary
        if matches!(node.label.as_str(), "Class" | "Interface") {
            if let Ok(file_nodes) = store.get_nodes_for_file(&project, &node.file_path) {
                let members: Vec<Value> = file_nodes.iter()
                    .filter(|n| n.id != node.id && n.start_line >= node.start_line && n.end_line <= node.end_line)
                    .map(|n| {
                        let mut m = json!({"name": n.name, "label": n.label, "start_line": n.start_line, "end_line": n.end_line});
                        if let Some(ref pj) = n.properties_json {
                            if let Ok(pv) = serde_json::from_str::<Value>(pj) {
                                if let Some(rt) = pv.get("return_type") { m["return_type"] = rt.clone(); }
                                if let Some(an) = pv.get("annotations") { m["annotations"] = an.clone(); }
                            }
                        }
                        m
                    }).collect();
                if !members.is_empty() {
                    resp["members"] = json!(members);
                }
            }
        }

        // Optional snippet — uses full AST node range, capped at 50 for classes, 150 for others
        if args.include_snippet.unwrap_or(true) && !node.file_path.is_empty() {
            let projects = store.list_projects().unwrap_or_default();
            if let Some(root) = projects
                .iter()
                .find(|p| p.name == project)
                .map(|p| &p.root_path)
            {
                let full = Path::new(root).join(&node.file_path);
                let node_start = node.start_line;
                let node_end_line = node.end_line;
                let node_label = node.label.clone();
                let snippet_lines_arg = args.snippet_lines;
                if let Ok(Ok(content)) =
                    tokio::task::spawn_blocking(move || std::fs::read_to_string(&full)).await
                {
                    let lines: Vec<&str> = content.lines().collect();
                    let s = (node_start.max(1) as usize).saturating_sub(1);
                    let node_end = (node_end_line as usize).min(lines.len());
                    let default_cap = if matches!(node_label.as_str(), "Class" | "Interface") {
                        50
                    } else {
                        150
                    };
                    let max_cap = snippet_lines_arg.map(|l| l as usize).unwrap_or(default_cap);
                    let e = node_end.min(s + max_cap);
                    resp["snippet"] = json!({
                        "start_line": s + 1, "end_line": e,
                        "content": lines[s..e].join("\n"),
                    });
                }
            }
        }

        let result = resp.to_string();
        self.analytics_log(&ctx, "get_symbol_details", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Find all references to a symbol (callers, importers). Better than text search — uses graph edges. \
                       Results include confidence and edge_source for each reference. \
                       Use min_confidence to filter low-confidence (e.g. regex-derived) edges."
    )]
    async fn find_references(
        &self,
        Parameters(args): Parameters<FindReferencesArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(30);
        let ref_type = args.reference_type.as_deref().unwrap_or("all");
        let group_by = args.group_by.as_deref().unwrap_or("file");
        let include_linked = args.include_linked.unwrap_or(false);
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "find_references", &project, start, &req, &r);
                return r;
            }
        };

        // Resolve target
        let node = if let Some(qn) = &args.qualified_name {
            match store.find_node_by_qn(&project, qn) {
                Ok(Some(n)) => n,
                Ok(None) => {
                    let r = json!({"error": format!("Not found: {}", qn)}).to_string();
                    self.analytics_log(&ctx, "find_references", &project, start, &req, &r);
                    return r;
                }
                Err(e) => {
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "find_references", &project, start, &req, &r);
                    return r;
                }
            }
        } else if let Some(name) = &args.name {
            match store.find_symbol_ranked(&project, name, args.label.as_deref(), false, 1) {
                Ok(ref m) if !m.is_empty() => m[0].0.clone(),
                Ok(_) => {
                    let r = json!({"error": format!("Not found: {}", name)}).to_string();
                    self.analytics_log(&ctx, "find_references", &project, start, &req, &r);
                    return r;
                }
                Err(e) => {
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "find_references", &project, start, &req, &r);
                    return r;
                }
            }
        } else {
            let r = json!({"error": "Provide qualified_name or name"}).to_string();
            self.analytics_log(&ctx, "find_references", &project, start, &req, &r);
            return r;
        };

        let edge_filter: Option<Vec<&str>> = match ref_type {
            "calls" => Some(vec!["CALLS", "ASYNC_CALLS", "HTTP_CALLS"]),
            "imports" => Some(vec!["IMPORTS"]),
            "uses" => Some(vec!["USES"]),
            _ => None,
        };
        let min_confidence = args.min_confidence;
        let refs = store
            .incoming_references_detailed(node.id, edge_filter.as_deref(), limit, min_confidence)
            .unwrap_or_default();

        // Cross-project references
        let mut linked_refs: Vec<Value> = Vec::new();
        if include_linked {
            let links = store.get_linked_projects(&project).unwrap_or_default();
            let search_name = args.name.as_deref().unwrap_or(&node.name);
            for link in &links {
                let remaining = limit - refs.len() as i32 - linked_refs.len() as i32;
                if remaining <= 0 {
                    break;
                }
                // Find the same symbol in the linked project
                if let Ok(linked_node) =
                    store.find_symbol_ranked(&link.target_project, search_name, None, false, 1)
                {
                    for (ln, _, _) in &linked_node {
                        if let Ok(lr) = store.incoming_references_detailed(
                            ln.id,
                            edge_filter.as_deref(),
                            remaining,
                            min_confidence,
                        ) {
                            for (src, et, conf, es) in &lr {
                                let mut entry = json!({
                                    "source_name": src.name,
                                    "source_qualified_name": src.qualified_name,
                                    "label": src.label,
                                    "file_path": src.file_path,
                                    "line": src.start_line,
                                    "edge_type": et,
                                    "source_project": link.target_project,
                                });
                                if let Some(c) = conf {
                                    entry["confidence"] = json!(c);
                                }
                                if let Some(s) = es {
                                    entry["edge_source"] = json!(s);
                                }
                                linked_refs.push(entry);
                            }
                        }
                    }
                }
            }
        }

        let result = if group_by == "file" {
            let mut groups: std::collections::BTreeMap<String, Vec<Value>> =
                std::collections::BTreeMap::new();
            for (src, et, conf, es) in &refs {
                let mut entry = json!({
                    "source_name": src.name, "source_qualified_name": src.qualified_name,
                    "label": src.label, "line": src.start_line, "edge_type": et,
                });
                if let Some(c) = conf {
                    entry["confidence"] = json!(c);
                }
                if let Some(s) = es {
                    entry["edge_source"] = json!(s);
                }
                groups.entry(src.file_path.clone()).or_default().push(entry);
            }
            let groups_json: Vec<Value> = groups
                .into_iter()
                .map(|(fp, r)| json!({"file_path": fp, "references": r}))
                .collect();
            let count = refs.len() + linked_refs.len();
            let has_more = count as i32 >= limit;
            let mut resp = json!({"project": project, "target": {"name": node.name, "qualified_name": node.qualified_name, "label": node.label}, "reference_type": ref_type, "groups": groups_json, "count": count, "has_more": has_more});
            if !linked_refs.is_empty() {
                resp["linked_references"] = json!(linked_refs);
            }
            // Warn if many results are low-confidence
            let low_conf_count = refs
                .iter()
                .filter(|(_, _, conf, _)| conf.is_some_and(|c| c < 0.6))
                .count();
            if low_conf_count > 0 && low_conf_count * 2 > refs.len() {
                resp["warning"] = json!(format!(
                    "{} of {} references are low-confidence (< 0.6). Consider using min_confidence=0.6 to filter regex/heuristic edges.",
                    low_conf_count, refs.len()
                ));
            }
            resp
        } else {
            let items: Vec<Value> = refs.iter().map(|(src, et, conf, es)| {
                let mut entry = json!({
                    "source_name": src.name, "source_qualified_name": src.qualified_name,
                    "label": src.label, "file_path": src.file_path, "line": src.start_line, "edge_type": et,
                });
                if let Some(c) = conf {
                    entry["confidence"] = json!(c);
                }
                if let Some(s) = es {
                    entry["edge_source"] = json!(s);
                }
                entry
            }).collect();
            let count = items.len() + linked_refs.len();
            let has_more = count as i32 >= limit;
            let mut resp = json!({"project": project, "target": {"name": node.name, "qualified_name": node.qualified_name, "label": node.label}, "reference_type": ref_type, "references": items, "count": count, "has_more": has_more});
            if !linked_refs.is_empty() {
                resp["linked_references"] = json!(linked_refs);
            }
            resp
        };

        let r = result.to_string();
        self.analytics_log(&ctx, "find_references", &project, start, &req, &r);
        r
    }

    #[tool(
        description = "Analyze the blast radius of changing a symbol or file. Shows direct/indirect dependents, affected files, and risk level. \
                       Direct dependents include confidence and edge_source. \
                       Use min_confidence to filter low-confidence edges from the traversal."
    )]
    async fn impact_analysis(
        &self,
        Parameters(args): Parameters<ImpactAnalysisArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let max_depth = args.max_depth.unwrap_or(3);
        let limit = args.limit.unwrap_or(50);
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "impact_analysis", &project, start, &req, &r);
                return r;
            }
        };

        // Collect target nodes
        let targets: Vec<codryn_store::Node> = if let Some(qn) = &args.qualified_name {
            store
                .find_node_by_qn(&project, qn)
                .unwrap_or(None)
                .into_iter()
                .collect()
        } else if let Some(name) = &args.name {
            store
                .find_symbol_ranked(&project, name, None, false, 1)
                .unwrap_or_default()
                .into_iter()
                .map(|(n, _, _)| n)
                .collect()
        } else if let Some(fp) = &args.file_path {
            store
                .search_nodes_filtered(&project, fp, None, 50)
                .unwrap_or_default()
                .into_iter()
                .filter(|n| n.file_path == *fp)
                .collect()
        } else {
            let r = json!({"error": "Provide qualified_name, name, or file_path"}).to_string();
            self.analytics_log(&ctx, "impact_analysis", &project, start, &req, &r);
            return r;
        };

        if targets.is_empty() {
            let r = json!({"error": "Target not found"}).to_string();
            self.analytics_log(&ctx, "impact_analysis", &project, start, &req, &r);
            return r;
        }

        // Aggregate impact across all target nodes
        let mut all_direct = Vec::new();
        let mut all_indirect = Vec::new();
        let mut all_files = std::collections::BTreeSet::new();
        let mut seen_ids = std::collections::HashSet::new();

        let min_confidence = args.min_confidence;

        for t in &targets {
            if let Ok((direct, all, files)) =
                store.impact_bfs_with_confidence(t.id, max_depth, limit, min_confidence)
            {
                for d in direct {
                    if seen_ids.insert(d.id) {
                        all_direct.push(d);
                    }
                }
                for (n, depth) in all {
                    if depth > 1 && seen_ids.insert(n.id) {
                        all_indirect.push(n);
                    }
                }
                all_files.extend(files);
            }
        }

        // Modules = unique first path segments of affected files
        let modules: std::collections::BTreeSet<String> = all_files
            .iter()
            .filter_map(|f| f.split('/').next().map(String::from))
            .collect();

        // Cross-project
        let mut cross_hits = 0usize;
        if args.include_linked.unwrap_or(false) {
            let links = store.get_linked_projects(&project).unwrap_or_default();
            for t in &targets {
                for link in &links {
                    let refs = store
                        .search_nodes_filtered(&link.target_project, &t.name, None, 10)
                        .unwrap_or_default();
                    cross_hits += refs.len();
                }
            }
        }

        let direct_count = all_direct.len();
        let indirect_count = all_indirect.len();
        let file_count = all_files.len();
        let mod_count = modules.len();

        let risk = if cross_hits > 0 || direct_count > 10 || file_count > 10 {
            "high"
        } else if direct_count > 3 || file_count > 3 {
            "medium"
        } else {
            "low"
        };

        let target_info = if targets.len() == 1 {
            json!({"qualified_name": targets[0].qualified_name, "label": targets[0].label})
        } else {
            json!({"file_path": args.file_path, "symbols": targets.len()})
        };

        let direct_samples: Vec<Value> = {
            // Enrich direct samples with confidence from edges
            let mut samples = Vec::new();
            for t in &targets {
                if samples.len() >= 10 {
                    break;
                }
                let remaining = 10 - samples.len();
                if let Ok(detailed) =
                    store.direct_dependents_with_confidence(t.id, remaining as i32, min_confidence)
                {
                    for (node, edge_type, conf, es) in detailed {
                        if samples.len() >= 10 {
                            break;
                        }
                        let mut entry = json!({
                            "name": node.name,
                            "qualified_name": node.qualified_name,
                            "file_path": node.file_path,
                            "edge_type": edge_type,
                        });
                        if let Some(c) = conf {
                            entry["confidence"] = json!(c);
                        }
                        if let Some(s) = es {
                            entry["edge_source"] = json!(s);
                        }
                        samples.push(entry);
                    }
                }
            }
            samples
        };
        let indirect_samples: Vec<Value> = all_indirect
            .iter()
            .take(10)
            .map(|n| {
                json!({
                    "name": n.name, "qualified_name": n.qualified_name, "file_path": n.file_path,
                })
            })
            .collect();
        let file_paths: Vec<&String> = all_files.iter().take(20).collect();

        // Warn if min_confidence was not set and there are low-confidence direct dependents
        let low_conf_direct = direct_samples
            .iter()
            .filter(|e| {
                e.get("confidence")
                    .and_then(|v| v.as_f64())
                    .is_some_and(|c| c < 0.6)
            })
            .count();
        let warning = if min_confidence.is_none()
            && low_conf_direct > 0
            && low_conf_direct * 2 > direct_samples.len()
        {
            Some(format!(
                "{} of {} direct dependents are low-confidence (< 0.6). Consider using min_confidence=0.6 to filter regex/heuristic edges.",
                low_conf_direct, direct_samples.len()
            ))
        } else {
            None
        };

        let mut result_obj = json!({
            "project": project, "target": target_info,
            "summary": {
                "direct_dependents": direct_count, "indirect_dependents": indirect_count,
                "affected_files": file_count, "affected_modules": mod_count,
                "cross_project_hits": cross_hits, "risk_level": risk,
            },
            "direct_samples": direct_samples, "indirect_samples": indirect_samples,
            "affected_file_paths": file_paths,
        });
        if let Some(w) = warning {
            result_obj["warning"] = json!(w);
        }
        let result = result_obj.to_string();
        self.analytics_log(&ctx, "impact_analysis", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Debug why a file or symbol is missing or incomplete in the index. Shows indexing status, language detection, and diagnostics."
    )]
    async fn explain_index_result(
        &self,
        Parameters(args): Parameters<ExplainIndexResultArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "explain_index_result", &project, start, &req, &r);
                return r;
            }
        };

        let result = if let Some(fp) = &args.file_path {
            self.explain_file(&store, &project, fp)
        } else if let Some(qn) = &args.qualified_name {
            self.explain_symbol_qn(&store, &project, qn)
        } else if let Some(name) = &args.name {
            self.explain_symbol_name(&store, &project, name)
        } else {
            json!({"error": "Provide file_path, qualified_name, or name"}).to_string()
        };

        self.analytics_log(&ctx, "explain_index_result", &project, start, &req, &result);
        result
    }

    // ── Phase 3: Agent Navigation Tools ───────────────────

    #[tool(
        description = "Get a compact summary of a file: symbols, imports, exports, graph neighborhood. Helps decide if a file is worth opening."
    )]
    async fn get_file_overview(
        &self,
        Parameters(args): Parameters<navigation_tools::GetFileOverviewArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                match NavigationService::file_overview(
                    &store,
                    &project,
                    &args.file_path,
                    args.include_symbols.unwrap_or(true),
                    args.include_imports.unwrap_or(true),
                    args.include_exports.unwrap_or(true),
                    args.include_neighbors.unwrap_or(true),
                ) {
                    Ok(overview) => serde_json::to_string(&overview)
                        .unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()),
                    Err(e) => json!({"error": e.to_string()}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "get_file_overview", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Find likely entrypoints in a project or subsystem: main functions, route handlers, CLI commands, lambda handlers."
    )]
    async fn find_entrypoints(
        &self,
        Parameters(args): Parameters<navigation_tools::FindEntrypointsArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                match NavigationService::find_entrypoints(
                    &store,
                    &project,
                    args.scope.as_deref(),
                    args.entry_type.as_deref(),
                    args.limit.unwrap_or(10),
                ) {
                    Ok(res) => serde_json::to_string(&res)
                        .unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()),
                    Err(e) => json!({"error": e.to_string()}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "find_entrypoints", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Suggest the next best files or symbols to read from a given starting point, ranked by relevance to a goal (understand, debug, refactor, trace, test)."
    )]
    async fn suggest_next_reads(
        &self,
        Parameters(args): Parameters<navigation_tools::SuggestNextReadsArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                match NavigationService::suggest_next_reads(
                    &store,
                    &project,
                    args.qualified_name.as_deref(),
                    args.file_path.as_deref(),
                    args.goal.as_deref(),
                    args.limit.unwrap_or(10),
                ) {
                    Ok(res) => serde_json::to_string(&res)
                        .unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()),
                    Err(e) => json!({"error": e.to_string()}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "suggest_next_reads", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Trace likely data/request flow through the codebase. Detects architectural patterns like route→controller→service→repository."
    )]
    async fn trace_data_flow(
        &self,
        Parameters(args): Parameters<navigation_tools::TraceDataFlowArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                match FlowAnalysisService::trace_data_flow(
                    &store,
                    &project,
                    args.source.as_deref(),
                    args.target.as_deref(),
                    args.file_path.as_deref(),
                    args.flow_type.as_deref(),
                    args.max_depth.unwrap_or(5),
                    args.limit.unwrap_or(10),
                    args.include_linked.unwrap_or(false),
                ) {
                    Ok(res) => serde_json::to_string(&res)
                        .unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()),
                    Err(e) => json!({"error": e.to_string()}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "trace_data_flow", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Find relevant tests for a symbol, file, or module using naming conventions, folder patterns, and graph references."
    )]
    async fn find_tests_for_target(
        &self,
        Parameters(args): Parameters<navigation_tools::FindTestsForTargetArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                match TestDiscoveryService::find_tests(
                    &store,
                    &project,
                    args.qualified_name.as_deref(),
                    args.name.as_deref(),
                    args.file_path.as_deref(),
                    args.limit.unwrap_or(10),
                ) {
                    Ok(res) => serde_json::to_string(&res)
                        .unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()),
                    Err(e) => json!({"error": e.to_string()}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(
            &ctx,
            "find_tests_for_target",
            &project,
            start,
            &req,
            &result,
        );
        result
    }

    #[tool(
        description = "Suggest likely cross-project links based on shared types, naming patterns, and domain overlap."
    )]
    async fn suggest_project_links(
        &self,
        Parameters(args): Parameters<navigation_tools::SuggestProjectLinksArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project_name = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                match ProjectLinkingService::suggest_links(
                    &store,
                    args.project.as_deref(),
                    args.limit.unwrap_or(10),
                ) {
                    Ok(res) => serde_json::to_string(&res)
                        .unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()),
                    Err(e) => json!({"error": e.to_string()}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(
            &ctx,
            "suggest_project_links",
            &project_name,
            start,
            &req,
            &result,
        );
        result
    }

    #[tool(
        description = "Find REST/HTTP routes in the project with handler, request DTO, and response DTO. Structured API discovery."
    )]
    async fn find_routes(
        &self,
        Parameters(args): Parameters<navigation_tools::FindRoutesArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(20);
        let include_deleted = args.include_deleted.unwrap_or(false);
        let result = match self.get_store().await {
            Ok(store) => {
                match store.find_routes(
                    &project,
                    args.scope.as_deref(),
                    args.method.as_deref(),
                    limit,
                    include_deleted,
                ) {
                    Ok(routes) => {
                        let count = routes.len();
                        let has_more = count as i32 == limit;
                        json!({"project": project, "routes": routes, "count": count, "has_more": has_more}).to_string()
                    }
                    Err(e) => json!({"error": e.to_string()}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "find_routes", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Trace the full backend request flow from route entry to repository. Returns structured controller→service→repository chain with a renderable graph."
    )]
    async fn trace_backend_flow(
        &self,
        Parameters(args): Parameters<navigation_tools::TraceBackendFlowArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                match BackendFlowService::trace(
                    &store,
                    &project,
                    args.route_path.as_deref(),
                    args.handler.as_deref(),
                    args.http_method.as_deref(),
                    args.max_depth.unwrap_or(5),
                    args.include_linked.unwrap_or(false),
                ) {
                    Ok(res) => serde_json::to_string(&res)
                        .unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()),
                    Err(e) => json!({"error": e.to_string()}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "trace_backend_flow", &project, start, &req, &result);
        result
    }

    #[tool(description = "Find CI/CD pipelines in a project with stages, jobs, and dependencies")]
    async fn find_pipelines(
        &self,
        Parameters(args): Parameters<FindPipelinesArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                self.trigger_auto_reindex(&store, &project);
                match PipelineService::list_pipelines(&store, &project) {
                    Ok(pipelines) => {
                        let count = pipelines.len();
                        json!({ "project": project, "pipelines": pipelines, "count": count })
                            .to_string()
                    }
                    Err(e) => json!({ "error": e.to_string() }).to_string(),
                }
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "find_pipelines", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Find infrastructure resources (Terraform, Kubernetes, Docker, Helm) in a project"
    )]
    async fn find_infrastructure(
        &self,
        Parameters(args): Parameters<FindInfrastructureArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                self.trigger_auto_reindex(&store, &project);
                match PipelineService::list_infrastructure(
                    &store,
                    &project,
                    args.infra_type.as_deref(),
                ) {
                    Ok(resources) => {
                        let count = resources.len();
                        json!({ "project": project, "resources": resources, "count": count })
                            .to_string()
                    }
                    Err(e) => json!({ "error": e.to_string() }).to_string(),
                }
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "find_infrastructure", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Sample high-centrality nodes from the graph for quick orientation. Returns nodes sorted by total degree (fan_in + fan_out) descending. Use sort_by='cyclomatic' or sort_by='cognitive' to sort by complexity metrics instead. Use sort_by='hotspot' to sort by git_commits descending (most frequently changed files)."
    )]
    async fn sample_graph(
        &self,
        Parameters(args): Parameters<SampleGraphArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(20) as usize;
        let sort_by = args.sort_by.as_deref().unwrap_or("degree");
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "sample_graph", &project, start, &req, &r);
                return r;
            }
        };
        self.trigger_auto_reindex(&store, &project);

        // Complexity-based sorting
        if sort_by == "cyclomatic" || sort_by == "cognitive" {
            let query_start = Instant::now();
            let min_cyc = if sort_by == "cyclomatic" { 1 } else { 0 };
            let min_cog = if sort_by == "cognitive" { 1 } else { 0 };
            let rows = match store.query_complexity(&project, min_cyc, min_cog, limit as i64) {
                Ok(r) => r,
                Err(e) => {
                    self.record_query_duration(query_start);
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "sample_graph", &project, start, &req, &r);
                    return r;
                }
            };
            self.record_query_duration(query_start);

            let mut items: Vec<Value> = rows
                .iter()
                .map(|r| {
                    json!({
                        "name": r.name,
                        "qualified_name": r.qualified_name,
                        "file_path": r.file_path,
                        "start_line": r.start_line,
                        "cyclomatic_complexity": r.cyclomatic_complexity,
                        "cognitive_complexity": r.cognitive_complexity,
                    })
                })
                .collect();

            // Sort by the requested metric descending
            if sort_by == "cognitive" {
                items.sort_by(|a, b| {
                    let av = a["cognitive_complexity"].as_u64().unwrap_or(0);
                    let bv = b["cognitive_complexity"].as_u64().unwrap_or(0);
                    bv.cmp(&av)
                });
            }

            let count = items.len();
            let result =
                json!({"project": project, "sort_by": sort_by, "nodes": items, "count": count})
                    .to_string();
            self.analytics_log(&ctx, "sample_graph", &project, start, &req, &result);
            return result;
        }

        // Hotspot-based sorting (git history)
        if sort_by == "hotspot" {
            let query_start = Instant::now();
            let rows = match store.query_hotspots(&project, limit as i64) {
                Ok(r) => r,
                Err(e) => {
                    self.record_query_duration(query_start);
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "sample_graph", &project, start, &req, &r);
                    return r;
                }
            };
            self.record_query_duration(query_start);

            if rows.is_empty() {
                let result = json!({
                    "project": project,
                    "sort_by": "hotspot",
                    "nodes": [],
                    "count": 0,
                    "message": "Git history has not been indexed for this project. Run indexing with git history enabled to populate hotspot data."
                })
                .to_string();
                self.analytics_log(&ctx, "sample_graph", &project, start, &req, &result);
                return result;
            }

            let items: Vec<Value> = rows
                .iter()
                .map(|r| {
                    json!({
                        "name": r.name,
                        "qualified_name": r.qualified_name,
                        "label": r.label,
                        "file_path": r.file_path,
                        "start_line": r.start_line,
                        "git_commits": r.git_commits,
                        "git_authors": r.git_authors,
                        "git_last_modified": r.git_last_modified,
                    })
                })
                .collect();

            let count = items.len();
            let result =
                json!({"project": project, "sort_by": "hotspot", "nodes": items, "count": count})
                    .to_string();
            self.analytics_log(&ctx, "sample_graph", &project, start, &req, &result);
            return result;
        }

        // Default: degree-based sorting
        let query_start = Instant::now();
        let degrees = match store.node_degrees_bulk(&project) {
            Ok(d) => d,
            Err(e) => {
                self.record_query_duration(query_start);
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "sample_graph", &project, start, &req, &r);
                return r;
            }
        };

        let all_nodes = match store.get_all_nodes(&project) {
            Ok(n) => n,
            Err(e) => {
                self.record_query_duration(query_start);
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "sample_graph", &project, start, &req, &r);
                return r;
            }
        };
        self.record_query_duration(query_start);

        // Build (node, fan_in, fan_out) tuples, sorted by total degree desc
        let mut scored: Vec<(&codryn_store::Node, i32, i32)> = all_nodes
            .iter()
            .filter(|n| {
                !matches!(
                    n.label.as_str(),
                    "Module" | "File" | "Folder" | "Project" | "Package"
                )
            })
            .map(|n| {
                let (fan_in, fan_out) = degrees.get(&n.id).copied().unwrap_or((0, 0));
                (n, fan_in, fan_out)
            })
            .filter(|(_, fi, fo)| *fi + *fo > 0)
            .collect();

        scored.sort_by_key(|b| std::cmp::Reverse(b.1 + b.2));
        scored.truncate(limit);

        let items: Vec<Value> = scored
            .iter()
            .map(|(n, fan_in, fan_out)| {
                let mut entry = json!({
                    "name": n.name,
                    "label": n.label,
                    "qualified_name": n.qualified_name,
                    "file_path": n.file_path,
                    "fan_in": fan_in,
                    "fan_out": fan_out,
                    "total_degree": fan_in + fan_out,
                });
                // Include complexity metrics if available
                if let Some(ref pj) = n.properties_json {
                    if let Ok(pv) = serde_json::from_str::<Value>(pj) {
                        if let Some(cyc) = pv.get("cyclomatic_complexity") {
                            entry["cyclomatic_complexity"] = cyc.clone();
                        }
                        if let Some(cog) = pv.get("cognitive_complexity") {
                            entry["cognitive_complexity"] = cog.clone();
                        }
                    }
                }
                entry
            })
            .collect();

        let count = items.len();
        let result =
            json!({"project": project, "sort_by": "degree", "nodes": items, "count": count})
                .to_string();
        self.analytics_log(&ctx, "sample_graph", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Health check endpoint reporting server status, uptime, and store connectivity."
    )]
    async fn health_check(&self, meta: rmcp::model::Meta) -> String {
        let ctx = Self::extract_ctx(&meta, None);
        let start = Instant::now();

        let store_result = match self.get_store().await {
            Ok(s) => match s.list_projects() {
                Ok(projects) => Ok(projects.len()),
                Err(e) => Err(e.to_string()),
            },
            Err(e) => Err(e.to_string()),
        };

        let active_runs = self.auto_indexer.active_index_runs();
        let status = HealthStatus::check(self.start_time, store_result, active_runs);
        let result = serde_json::to_string(&status).unwrap_or_else(|_| "{}".to_string());
        self.analytics_log(&ctx, "health_check", "", start, "", &result);
        result
    }

    #[tool(
        description = "Get diagnostic information about the system: open file descriptors, query performance metrics, and health status."
    )]
    async fn diagnostics(&self, meta: rmcp::model::Meta) -> String {
        let ctx = Self::extract_ctx(&meta, None);
        let start = Instant::now();
        let report = self.diagnostics.report();
        let result = serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string());
        self.analytics_log(&ctx, "diagnostics", "", start, "", &result);
        result
    }

    // ── Track 5.3 — Project Summary ───────────────────────────────────────

    #[tool(
        description = "Get a structured onboarding brief for a project in one call. Returns language breakdown, architecture layers, top-10 high-centrality symbols, route count, entry points, linked projects, detected patterns (MVC, microservice, event-driven, etc.), and suggested first reads. Replaces 4–5 separate tool calls for project orientation."
    )]
    async fn get_project_summary(
        &self,
        Parameters(args): Parameters<GetProjectSummaryArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(s) => {
                self.trigger_auto_reindex(&s, &project);
                let query_start = Instant::now();
                let r = codryn_services::project_summary::ProjectSummaryService::get_summary(
                    &s, &project,
                );
                self.record_query_duration(query_start);
                match r {
                    Ok(summary) => serde_json::to_string(&summary).unwrap_or_default(),
                    Err(e) => json!({ "error": e.to_string() }).to_string(),
                }
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "get_project_summary", &project, start, &req, &result);
        result
    }

    // ── Track 5.9 — Context for Task ─────────────────────────────────────

    #[tool(
        description = "Get everything needed to work on a symbol in one call. Specify task type: 'modify' (callers + callees + imports + tests), 'debug' (call chain 2 levels deep), 'test' (existing tests + mock candidates + similar tested functions), 'document' (callers + usage examples + related symbols). Collapses 3–4 tool calls into one."
    )]
    async fn get_context_for_task(
        &self,
        Parameters(args): Parameters<GetContextForTaskArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let symbol_name = args.name.as_deref().unwrap_or("");
        let task = args.task.as_deref().unwrap_or("modify");
        let result = match self.get_store().await {
            Ok(s) => {
                self.trigger_auto_reindex(&s, &project);
                let query_start = Instant::now();
                let r = codryn_services::context_for_task::ContextForTaskService::get_context(
                    &s,
                    &project,
                    symbol_name,
                    args.qualified_name.as_deref(),
                    task,
                );
                self.record_query_duration(query_start);
                match r {
                    Ok(ctx_result) => serde_json::to_string(&ctx_result).unwrap_or_default(),
                    Err(e) => json!({ "error": e.to_string() }).to_string(),
                }
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "get_context_for_task", &project, start, &req, &result);
        result
    }

    // ── Track 5.12 — Batch Symbol Resolution ─────────────────────────────

    #[tool(
        description = "Resolve multiple symbols in one call. Pass 'names' array for up to 50 symbol names/qualified names. Or use 'filter' for structured queries: 'class:ClassName' returns all methods of a class, 'file:path/to/file.ts' returns all symbols in a file. Returns details (signature, complexity, doc coverage, fan-in/out) and optionally internal edges between the returned symbols."
    )]
    async fn get_symbols_batch(
        &self,
        Parameters(args): Parameters<GetSymbolsBatchArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let include_edges = args.include_internal_edges.unwrap_or(false);
        let result = match self.get_store().await {
            Ok(s) => {
                self.trigger_auto_reindex(&s, &project);
                let query_start = Instant::now();
                let r = if let Some(filter) = &args.filter {
                    if let Some(class_name) = filter.strip_prefix("class:") {
                        codryn_services::batch_symbols::BatchSymbolsService::get_class_members(
                            &s, &project, class_name,
                        )
                    } else if let Some(file_path) = filter.strip_prefix("file:") {
                        codryn_services::batch_symbols::BatchSymbolsService::get_file_symbols(
                            &s, &project, file_path,
                        )
                    } else {
                        Err(anyhow::anyhow!(
                            "Unknown filter format. Use 'class:ClassName' or 'file:path/to/file'"
                        ))
                    }
                } else if let Some(names) = &args.names {
                    codryn_services::batch_symbols::BatchSymbolsService::get_batch(
                        &s,
                        &project,
                        names,
                        include_edges,
                    )
                } else {
                    Err(anyhow::anyhow!(
                        "Provide either 'names' array or 'filter' parameter"
                    ))
                };
                self.record_query_duration(query_start);
                match r {
                    Ok(batch_result) => serde_json::to_string(&batch_result).unwrap_or_default(),
                    Err(e) => json!({ "error": e.to_string() }).to_string(),
                }
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "get_symbols_batch", &project, start, &req, &result);
        result
    }

    // ── Milestone 3 Tools ─────────────────────────────────────────────────

    #[tool(
        description = "Predict what breaks if you rename, remove, move, or change a symbol's signature. Returns breakages and a fix plan."
    )]
    async fn what_if(
        &self,
        Parameters(args): Parameters<WhatIfArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "what_if", &project, start, &req, &r);
                return r;
            }
        };
        let change_type = match args.change_type.as_str() {
            "rename" => what_if::ChangeType::Rename,
            "remove" => what_if::ChangeType::Remove,
            "change_signature" => what_if::ChangeType::ChangeSignature,
            "move_file" => what_if::ChangeType::MoveFile,
            other => {
                let r = json!({"error": format!("Unknown change_type: '{}'. Use: rename, remove, change_signature, move_file", other)}).to_string();
                self.analytics_log(&ctx, "what_if", &project, start, &req, &r);
                return r;
            }
        };
        let wi_req = what_if::WhatIfRequest {
            project: project.clone(),
            symbol: args.symbol,
            change_type,
            new_value: args.new_value,
            max_depth: args.max_depth,
        };
        let result = match what_if::WhatIfService::analyze(&store, &wi_req) {
            Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "what_if", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Find likely dead code: functions, methods, and classes with no incoming references."
    )]
    async fn find_dead_code(
        &self,
        Parameters(args): Parameters<FindDeadCodeArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "find_dead_code", &project, start, &req, &r);
                return r;
            }
        };
        let result =
            match dead_code::find_dead_code(&store, &project, args.scope.as_deref(), args.limit) {
                Ok(items) => {
                    let count = items.len();
                    json!({"project": project, "dead_code": items, "count": count}).to_string()
                }
                Err(e) => json!({"error": e.to_string()}).to_string(),
            };
        self.analytics_log(&ctx, "find_dead_code", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Get the dependency graph at file, folder, or package granularity. Detects cycles and returns topological order."
    )]
    async fn get_dependency_graph(
        &self,
        Parameters(args): Parameters<GetDependencyGraphArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "get_dependency_graph", &project, start, &req, &r);
                return r;
            }
        };
        let granularity = match args.granularity.as_deref() {
            Some("file") => dependency_graph::Granularity::File,
            Some("package") => dependency_graph::Granularity::Package,
            _ => dependency_graph::Granularity::Folder,
        };
        let result = match dependency_graph::get_dependency_graph(
            &store,
            &project,
            granularity,
            args.scope.as_deref(),
            args.include_cycles_only.unwrap_or(false),
        ) {
            Ok(graph) => serde_json::to_string(&graph).unwrap_or_default(),
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "get_dependency_graph", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Check how stale the index is by comparing file hashes. Returns a staleness score and list of changed files."
    )]
    async fn freshness_check(
        &self,
        Parameters(args): Parameters<FreshnessCheckArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "freshness_check", &project, start, &req, &r);
                return r;
            }
        };
        let repo_root = {
            let guard = self.session_root.lock().await;
            guard.clone().unwrap_or_else(|| ".".to_string())
        };
        let result = match staleness::compute_staleness(
            &store,
            &project,
            Path::new(&repo_root),
            args.scope.as_deref(),
        ) {
            Ok(report) => serde_json::to_string(&report).unwrap_or_default(),
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "freshness_check", &project, start, &req, &result);
        result
    }

    #[tool(description = "Clear the query cache. Returns status ok.")]
    async fn clear_cache(
        &self,
        Parameters(_args): Parameters<ClearCacheArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, _args.analytics.as_ref());
        self.query_cache.invalidate_all();
        let result = json!({"status": "ok"}).to_string();
        self.analytics_log(&ctx, "clear_cache", "", start, "", &result);
        result
    }

    #[tool(description = "Ask a natural language question about the codebase graph")]
    async fn ask_graph(
        &self,
        Parameters(args): Parameters<AskGraphArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                match NLToCypherService::translate_and_execute(&store, &project, &args.question) {
                    Ok(r) => serde_json::to_string(&r).unwrap_or_default(),
                    Err(e) => json!({"error": e.to_string()}).to_string(),
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "ask_graph", &project, start, &req, &result);
        result
    }

    #[tool(description = "Generate a step-by-step refactoring plan for a symbol or file")]
    async fn plan_refactoring(
        &self,
        Parameters(args): Parameters<PlanRefactoringArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let rtype = match args.refactoring_type.as_str() {
            "extract_module" => RefactoringType::ExtractModule,
            "split_class" => RefactoringType::SplitClass,
            "inline_function" => RefactoringType::InlineFunction,
            "extract_interface" => RefactoringType::ExtractInterface,
            _ => RefactoringType::MoveFunction,
        };
        let result = match self.get_store().await {
            Ok(store) => match RefactoringService::plan(&store, &project, &args.target, rtype) {
                Ok(plan) => serde_json::to_string(&plan).unwrap_or_default(),
                Err(e) => json!({"error": e.to_string()}).to_string(),
            },
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "plan_refactoring", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Review changed files for potential issues: uncovered callers, missing test updates"
    )]
    async fn review_changes(
        &self,
        Parameters(args): Parameters<ReviewChangesArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => match diff_review::DiffReviewService::review_changes(
                &store,
                &project,
                &args.changed_files,
            ) {
                Ok(r) => serde_json::to_string(&r).unwrap_or_default(),
                Err(e) => json!({"error": e.to_string()}).to_string(),
            },
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "review_changes", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Detect design patterns and antipatterns (MVC, Singleton, God Class, circular deps)"
    )]
    async fn detect_patterns(
        &self,
        Parameters(args): Parameters<DetectPatternsArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => match pattern_detection::PatternDetectionService::detect_patterns(
                &store,
                &project,
                args.patterns_only.unwrap_or(false),
                args.antipatterns_only.unwrap_or(false),
            ) {
                Ok(r) => serde_json::to_string(&r).unwrap_or_default(),
                Err(e) => json!({"error": e.to_string()}).to_string(),
            },
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "detect_patterns", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Find untested symbols ranked by risk (fan-in), with module coverage breakdown"
    )]
    async fn test_coverage_map(
        &self,
        Parameters(args): Parameters<TestCoverageMapArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => match test_gap::TestGapService::test_coverage_map(
                &store,
                &project,
                args.scope.as_deref(),
                args.untested_only.unwrap_or(false),
                args.limit.unwrap_or(50),
            ) {
                Ok(r) => serde_json::to_string(&r).unwrap_or_default(),
                Err(e) => json!({"error": e.to_string()}).to_string(),
            },
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "test_coverage_map", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Trace error propagation from a symbol through its callers, finding uncaught paths"
    )]
    async fn trace_error_flow(
        &self,
        Parameters(args): Parameters<TraceErrorFlowArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => match error_chain::ErrorChainService::trace_error_flow(
                &store,
                &project,
                &args.symbol,
                args.max_depth.unwrap_or(5),
            ) {
                Ok(r) => serde_json::to_string(&r).unwrap_or_default(),
                Err(e) => json!({"error": e.to_string()}).to_string(),
            },
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "trace_error_flow", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Get the public API surface of a project: exported symbols with signatures and docs"
    )]
    async fn get_api_surface(
        &self,
        Parameters(args): Parameters<GetAPISurfaceArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => {
                // Handle diff=true: compare current routes against most recent snapshot
                if args.diff.unwrap_or(false) {
                    match codryn_services::openapi::diff_api_surface(&store, &project) {
                        Ok(r) => serde_json::to_string(&r).unwrap_or_default(),
                        Err(e) => json!({"error": e.to_string()}).to_string(),
                    }
                } else {
                    match api_surface::APISurfaceService::get_api_surface(
                        &store,
                        &project,
                        args.module_filter.as_deref(),
                        args.symbol_type.as_deref(),
                        args.limit.unwrap_or(50),
                        args.undocumented.unwrap_or(false),
                    ) {
                        Ok(r) => serde_json::to_string(&r).unwrap_or_default(),
                        Err(e) => json!({"error": e.to_string()}).to_string(),
                    }
                }
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "get_api_surface", &project, start, &req, &result);
        result
    }

    #[tool(
        description = "Compare two graph snapshots to detect structural drift between index runs. Returns node/edge deltas and per-label/per-edge-type changes."
    )]
    async fn get_graph_diff(
        &self,
        Parameters(args): Parameters<GetGraphDiffArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let req = serde_json::to_string(&args).unwrap_or_default();
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "get_graph_diff", &project, start, &req, &r);
                return r;
            }
        };

        // Determine which snapshot IDs to compare
        let (from_id, to_id) = match (args.from_snapshot_id, args.to_snapshot_id) {
            (Some(from), Some(to)) => (from, to),
            _ => {
                // Default: compare the two most recent snapshots
                let snapshots = match store.list_snapshots(&project, 2) {
                    Ok(s) => s,
                    Err(e) => {
                        let r = json!({"error": e.to_string()}).to_string();
                        self.analytics_log(&ctx, "get_graph_diff", &project, start, &req, &r);
                        return r;
                    }
                };
                if snapshots.len() < 2 {
                    let r = json!({
                        "error": "Insufficient snapshots for comparison. At least 2 snapshots are required.",
                        "available_snapshots": snapshots.len()
                    })
                    .to_string();
                    self.analytics_log(&ctx, "get_graph_diff", &project, start, &req, &r);
                    return r;
                }
                // list_snapshots returns most recent first: [newest, second_newest, ...]
                let to = snapshots[0].id;
                let from = snapshots[1].id;
                (from, to)
            }
        };

        // Compute the diff
        let result = match store.diff_snapshots(from_id, to_id) {
            Ok(diff) => {
                // Fetch snapshot metadata for the response
                let from_snap = store
                    .list_snapshots(&project, 100)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|s| s.id == from_id);
                let to_snap = store
                    .list_snapshots(&project, 100)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|s| s.id == to_id);

                let mut response = json!({
                    "from_snapshot_id": diff.from_snapshot_id,
                    "to_snapshot_id": diff.to_snapshot_id,
                    "node_delta": diff.node_delta,
                    "edge_delta": diff.edge_delta,
                    "label_changes": diff.label_changes,
                    "edge_type_changes": diff.edge_type_changes,
                });

                if let Some(ref snap) = from_snap {
                    response["from_timestamp"] = json!(snap.timestamp);
                    response["from_content_hash"] = json!(snap.content_hash);
                }
                if let Some(ref snap) = to_snap {
                    response["to_timestamp"] = json!(snap.timestamp);
                    response["to_content_hash"] = json!(snap.content_hash);
                }

                response.to_string()
            }
            Err(e) => {
                // Check if the error is about a snapshot not being found
                let err_msg = e.to_string();
                if err_msg.contains("snapshot not found") {
                    json!({
                        "error": format!("Invalid snapshot ID: {}", err_msg)
                    })
                    .to_string()
                } else {
                    json!({"error": err_msg}).to_string()
                }
            }
        };

        self.analytics_log(&ctx, "get_graph_diff", &project, start, &req, &result);
        result
    }
}

#[tool_handler]
impl ServerHandler for CodrynServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            instructions: Some(
                "Persistent codebase knowledge graph — 66 languages, sub-ms queries. \
                 Use index_repository to index a project, then search_graph or query_graph to explore. \
                 Use link_project to connect related projects (e.g. frontend↔backend), then \
                 search_linked_projects to query across them. trace_call_path and query_graph \
                 (with include_linked=true) also work cross-project. \
                 Use find_symbol for fast symbol lookup, get_symbol_details for full context, \
                 find_references for usage analysis, impact_analysis for blast radius, \
                 and explain_index_result for debugging indexing issues. \
                 Navigation tools: get_file_overview for compact file summaries, \
                 find_entrypoints to discover where to start reading, \
                 suggest_next_reads for ranked next-step recommendations, \
                 trace_data_flow for request/data flow discovery, \
                 find_tests_for_target to locate relevant tests, \
                 suggest_project_links to discover cross-project connections, \
                 trace_backend_flow to explain full backend request flows (route→controller→service→repository). \
                 Use find_pipelines to discover CI/CD pipelines with stages, jobs, and dependencies. \
                 Use find_infrastructure to discover infrastructure resources (Terraform, Kubernetes, Docker, Helm). \
                 Agent-first tools: get_project_summary for a full onboarding brief in one call, \
                 get_context_for_task for everything needed to modify/debug/test/document a symbol, \
                 get_symbols_batch to resolve multiple symbols at once (or all members of a class/file). \
                 Use diagnostics to check system health, open file descriptors, and query performance metrics. \
                 Use health_check for a lightweight server status probe (uptime, store connectivity, active index runs)."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
