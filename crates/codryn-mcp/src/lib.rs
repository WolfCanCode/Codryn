mod args;
mod helpers;
mod navigation_tools;

pub use args::*;

use codryn_foundation::fqn;
use codryn_pipeline::{IndexMode, Pipeline};
use codryn_services::analytics::{AnalyticsContext, AnalyticsService};
use codryn_services::architecture::ArchitectureService;
use codryn_services::backend_flow::BackendFlowService;
use codryn_services::flow::FlowAnalysisService;
use codryn_services::navigation::NavigationService;
use codryn_services::project_linking::ProjectLinkingService;
use codryn_services::test_discovery::TestDiscoveryService;
use codryn_store::Store;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::Parameters;
use rmcp::model::{ProtocolVersion, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use serde_json::{json, Value};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct CodrynServer {
    store_path: PathBuf,
    session_root: Arc<Mutex<Option<String>>>,
    tool_router: ToolRouter<Self>,
}

impl CodrynServer {
    pub fn new(store_path: &Path) -> Self {
        Self {
            store_path: store_path.to_owned(),
            session_root: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    async fn get_store(&self) -> anyhow::Result<Store> {
        if self.store_path.to_string_lossy() == ":memory:" {
            Store::open_in_memory()
        } else {
            std::fs::create_dir_all(&self.store_path)?;
            Store::open(&self.store_path.join("graph.db"))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn analytics_log(
        &self,
        ctx: &AnalyticsContext,
        tool: &str,
        project: &str,
        start: Instant,
        result: &str,
    ) {
        let duration_ms = start.elapsed().as_millis() as i64;
        AnalyticsService::log_call(&self.store_path, ctx, tool, project, duration_ms, result);
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
}

#[tool_router]
impl CodrynServer {
    #[tool(description = "List all indexed projects with their metadata")]
    async fn list_projects(&self, meta: rmcp::model::Meta) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, None);
        let result = match self.get_store().await.and_then(|s| s.list_projects()) {
            Ok(projects) => serde_json::to_string(&json!({ "projects": projects })).unwrap(),
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "list_projects", "", start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self
            .get_store()
            .await
            .and_then(|s| s.get_graph_schema(&project))
        {
            Ok(schema) => serde_json::to_string(&schema).unwrap(),
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "get_graph_schema", &project, start, &result);
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
        let mode = match args.mode.as_deref() {
            Some("fast") => IndexMode::Fast,
            _ => IndexMode::Full,
        };
        let repo = PathBuf::from(&args.path);
        let store_path = self.store_path.clone();
        let project_name = Pipeline::new(&repo, &store_path, mode).project_name();

        let run_result =
            tokio::task::spawn_blocking(move || Pipeline::new(&repo, &store_path, mode).run())
                .await;

        let result = match run_result {
            Ok(Ok(())) => {
                let mut guard = self.session_root.lock().await;
                *guard = Some(args.path.clone());
                json!({ "status": "ok", "project": project_name }).to_string()
            }
            Ok(Err(e)) => json!({ "error": e.to_string() }).to_string(),
            Err(e) => json!({ "error": format!("task panicked: {}", e) }).to_string(),
        };
        self.analytics_log(&ctx, "index_repository", &project_name, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(20);
        let result = match self.get_store().await {
            Ok(s) => {
                let nodes = s
                    .search_nodes_broad(&project, &args.query, None, limit)
                    .unwrap_or_default();
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
        self.analytics_log(&ctx, "search_graph", &project, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "query_graph", &project, start, &r);
                return r;
            }
        };
        let local = match codryn_cypher::execute(&store, &project, &args.query) {
            Ok(r) => r,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "query_graph", &project, start, &r);
                return r;
            }
        };

        if !args.include_linked.unwrap_or(false) {
            let r = local.to_string();
            self.analytics_log(&ctx, "query_graph", &project, start, &r);
            return r;
        }

        let links = store.get_linked_projects(&project).unwrap_or_default();
        if links.is_empty() {
            let r = local.to_string();
            self.analytics_log(&ctx, "query_graph", &project, start, &r);
            return r;
        }

        let mut all_results = vec![json!({ "project": project, "result": local })];
        for link in &links {
            if let Ok(r) = codryn_cypher::execute(&store, &link.target_project, &args.query) {
                all_results.push(json!({ "project": link.target_project, "result": r }))
            }
        }
        let result = json!({ "results": all_results }).to_string();
        self.analytics_log(&ctx, "query_graph", &project, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self
            .get_store()
            .await
            .and_then(|s| s.get_graph_schema(&project))
        {
            Ok(schema) => json!({
                "project": project,
                "indexed": schema.total_nodes > 0,
                "total_nodes": schema.total_nodes,
                "total_edges": schema.total_edges,
            })
            .to_string(),
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "index_status", &project, start, &result);
        result
    }

    #[tool(description = "Trace a call path between two functions")]
    async fn trace_call_path(
        &self,
        Parameters(args): Parameters<TraceArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let project = self.resolve_project(args.project.as_deref()).await;
        let max_depth = args.max_depth.unwrap_or(5);
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "trace_call_path", &project, start, &r);
                return r;
            }
        };

        let target = if args.target.is_empty() {
            None
        } else {
            Some(args.target.as_str())
        };
        let steps = match store.trace_calls(&project, &args.source, target, max_depth) {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "trace_call_path", &project, start, &r);
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
            self.analytics_log(&ctx, "trace_call_path", &project, start, &r);
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
        self.analytics_log(&ctx, "trace_call_path", &project, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let result = match self.get_store().await {
            Ok(store) => match ArchitectureService::get_architecture(&store, &project) {
                Ok(arch) => serde_json::to_string(&arch)
                    .unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()),
                Err(e) => json!({"error": e.to_string()}).to_string(),
            },
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "get_architecture", &project, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "get_code_snippet", &project, timer, &r);
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
            match std::fs::read_to_string(&full_path) {
                Ok(content) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let start = args.start_line.unwrap_or(1).max(1) as usize - 1;
                    let end = args
                        .end_line
                        .map(|e| e as usize)
                        .unwrap_or(lines.len())
                        .min(lines.len());
                    let snippet: Vec<&str> = lines[start..end].to_vec();
                    json!({
                        "file_path": args.file_path,
                        "start_line": start + 1,
                        "end_line": end,
                        "content": snippet.join("\n"),
                    })
                    .to_string()
                }
                Err(e) => json!({ "error": e.to_string() }).to_string(),
            }
        };
        self.analytics_log(&ctx, "get_code_snippet", &project, timer, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(20) as usize;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "search_code", &project, start, &r);
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
        let mut results = Vec::new();
        for file_path in &files {
            if results.len() >= limit {
                break;
            }
            let full = Path::new(&root).join(file_path);
            if let Ok(content) = std::fs::read_to_string(&full) {
                for (i, line) in content.lines().enumerate() {
                    if results.len() >= limit {
                        break;
                    }
                    if line.contains(&args.pattern) {
                        results.push(json!({
                            "file": file_path,
                            "line": i + 1,
                            "content": line.trim(),
                        }));
                    }
                }
            }
        }
        let count = results.len();
        let has_more = count >= limit;
        let result =
            json!({ "matches": results, "count": count, "has_more": has_more }).to_string();
        self.analytics_log(&ctx, "search_code", &project, start, &result);
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

        let result = match std::process::Command::new("git")
            .args(["diff", "--name-only", "HEAD"])
            .current_dir(&root)
            .output()
        {
            Ok(output) => {
                let files: Vec<&str> = std::str::from_utf8(&output.stdout)
                    .unwrap_or("")
                    .lines()
                    .filter(|l| !l.is_empty())
                    .collect();
                json!({ "changed_files": files, "count": files.len() }).to_string()
            }
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "detect_changes", &project, start, &result);
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
        let result = match self
            .get_store()
            .await
            .and_then(|s| s.delete_project(&args.project))
        {
            Ok(()) => json!({ "status": "deleted", "project": args.project }).to_string(),
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        };
        self.analytics_log(&ctx, "delete_project", &args.project, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "manage_adr", &project, start, &r);
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
                    Ok(Some(adr)) => serde_json::to_string(&adr).unwrap(),
                    Ok(None) => json!({ "error": "ADR not found" }).to_string(),
                    Err(e) => json!({ "error": e.to_string() }).to_string(),
                }
            }
            _ => json!({ "error": "action must be 'list', 'create', or 'get'" }).to_string(),
        };
        self.analytics_log(&ctx, "manage_adr", &project, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "ingest_traces", &project, start, &r);
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
        self.analytics_log(&ctx, "ingest_traces", &project, start, &result);
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
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({ "error": e.to_string() }).to_string();
                self.analytics_log(&ctx, "link_project", "", start, &r);
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
        self.analytics_log(&ctx, "link_project", &args.project, start, &result);
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
        self.analytics_log(&ctx, "list_project_links", &project, start, &result);
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
        self.analytics_log(&ctx, "search_linked_projects", &project, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(10);
        let exact = args.exact.unwrap_or(false);
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "find_symbol", &project, start, &r);
                return r;
            }
        };
        let result = match store.find_symbol_ranked(
            &project,
            &args.query,
            args.label.as_deref(),
            exact,
            limit,
        ) {
            Ok(matches) => {
                let projects = store.list_projects().unwrap_or_default();
                let root = projects
                    .iter()
                    .find(|p| p.name == project)
                    .map(|p| p.root_path.as_str())
                    .unwrap_or("");
                let items: Vec<Value> = matches.iter().map(|(n, mt, sc)| {
                    let exists = !n.file_path.is_empty() && Path::new(root).join(&n.file_path).exists();
                    json!({
                        "name": n.name, "qualified_name": n.qualified_name, "label": n.label,
                        "file_path": n.file_path, "start_line": n.start_line, "end_line": n.end_line,
                        "match_type": mt, "score": sc, "exists": exists,
                    })
                }).collect();
                let count = items.len();
                let has_more = count as i32 == limit;
                json!({"project": project, "matches": items, "count": count, "has_more": has_more})
                    .to_string()
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        };
        self.analytics_log(&ctx, "find_symbol", &project, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "get_symbol_details", &project, start, &r);
                return r;
            }
        };

        // Resolve symbol
        let (node, alternatives) = if let Some(qn) = &args.qualified_name {
            match store.find_node_by_qn(&project, qn) {
                Ok(Some(n)) => (n, vec![]),
                Ok(None) => {
                    let r = json!({"error": format!("Symbol not found: {}", qn)}).to_string();
                    self.analytics_log(&ctx, "get_symbol_details", &project, start, &r);
                    return r;
                }
                Err(e) => {
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "get_symbol_details", &project, start, &r);
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
                    self.analytics_log(&ctx, "get_symbol_details", &project, start, &r);
                    return r;
                }
                Err(e) => {
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "get_symbol_details", &project, start, &r);
                    return r;
                }
            }
        } else {
            let r = json!({"error": "Provide qualified_name or name"}).to_string();
            self.analytics_log(&ctx, "get_symbol_details", &project, start, &r);
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

        // Optional snippet — uses full AST node range, capped at 150 lines
        if args.include_snippet.unwrap_or(true) && !node.file_path.is_empty() {
            let projects = store.list_projects().unwrap_or_default();
            if let Some(root) = projects
                .iter()
                .find(|p| p.name == project)
                .map(|p| &p.root_path)
            {
                let full = Path::new(root).join(&node.file_path);
                if let Ok(content) = std::fs::read_to_string(&full) {
                    let lines: Vec<&str> = content.lines().collect();
                    let s = (node.start_line.max(1) as usize).saturating_sub(1);
                    let node_end = (node.end_line as usize).min(lines.len());
                    let max_cap = args.snippet_lines.map(|l| l as usize).unwrap_or(150);
                    let e = node_end.min(s + max_cap);
                    resp["snippet"] = json!({
                        "start_line": s + 1, "end_line": e,
                        "content": lines[s..e].join("\n"),
                    });
                }
            }
        }

        let result = resp.to_string();
        self.analytics_log(&ctx, "get_symbol_details", &project, start, &result);
        result
    }

    #[tool(
        description = "Find all references to a symbol (callers, importers). Better than text search — uses graph edges."
    )]
    async fn find_references(
        &self,
        Parameters(args): Parameters<FindReferencesArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let project = self.resolve_project(args.project.as_deref()).await;
        let limit = args.limit.unwrap_or(30);
        let ref_type = args.reference_type.as_deref().unwrap_or("all");
        let group_by = args.group_by.as_deref().unwrap_or("file");
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "find_references", &project, start, &r);
                return r;
            }
        };

        // Resolve target
        let node = if let Some(qn) = &args.qualified_name {
            match store.find_node_by_qn(&project, qn) {
                Ok(Some(n)) => n,
                Ok(None) => {
                    let r = json!({"error": format!("Not found: {}", qn)}).to_string();
                    self.analytics_log(&ctx, "find_references", &project, start, &r);
                    return r;
                }
                Err(e) => {
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "find_references", &project, start, &r);
                    return r;
                }
            }
        } else if let Some(name) = &args.name {
            match store.find_symbol_ranked(&project, name, args.label.as_deref(), false, 1) {
                Ok(ref m) if !m.is_empty() => m[0].0.clone(),
                Ok(_) => {
                    let r = json!({"error": format!("Not found: {}", name)}).to_string();
                    self.analytics_log(&ctx, "find_references", &project, start, &r);
                    return r;
                }
                Err(e) => {
                    let r = json!({"error": e.to_string()}).to_string();
                    self.analytics_log(&ctx, "find_references", &project, start, &r);
                    return r;
                }
            }
        } else {
            let r = json!({"error": "Provide qualified_name or name"}).to_string();
            self.analytics_log(&ctx, "find_references", &project, start, &r);
            return r;
        };

        let edge_filter: Option<Vec<&str>> = match ref_type {
            "calls" => Some(vec!["CALLS", "ASYNC_CALLS", "HTTP_CALLS"]),
            "imports" => Some(vec!["IMPORTS"]),
            _ => None,
        };
        let refs = store
            .incoming_references(node.id, edge_filter.as_deref(), limit)
            .unwrap_or_default();

        let result = if group_by == "file" {
            let mut groups: std::collections::BTreeMap<String, Vec<Value>> =
                std::collections::BTreeMap::new();
            for (src, et) in &refs {
                groups
                    .entry(src.file_path.clone())
                    .or_default()
                    .push(json!({
                        "source_name": src.name, "source_qualified_name": src.qualified_name,
                        "line": src.start_line, "edge_type": et,
                    }));
            }
            let groups_json: Vec<Value> = groups
                .into_iter()
                .map(|(fp, r)| json!({"file_path": fp, "references": r}))
                .collect();
            let count = refs.len();
            let has_more = count as i32 == limit;
            json!({"project": project, "target": {"name": node.name, "qualified_name": node.qualified_name, "label": node.label}, "reference_type": ref_type, "groups": groups_json, "count": count, "has_more": has_more})
        } else {
            let items: Vec<Value> = refs.iter().map(|(src, et)| json!({
                "source_name": src.name, "source_qualified_name": src.qualified_name,
                "label": src.label, "file_path": src.file_path, "line": src.start_line, "edge_type": et,
            })).collect();
            let count = items.len();
            let has_more = count as i32 == limit;
            json!({"project": project, "target": {"name": node.name, "qualified_name": node.qualified_name, "label": node.label}, "reference_type": ref_type, "references": items, "count": count, "has_more": has_more})
        };

        let r = result.to_string();
        self.analytics_log(&ctx, "find_references", &project, start, &r);
        r
    }

    #[tool(
        description = "Analyze the blast radius of changing a symbol or file. Shows direct/indirect dependents, affected files, and risk level."
    )]
    async fn impact_analysis(
        &self,
        Parameters(args): Parameters<ImpactAnalysisArgs>,
        meta: rmcp::model::Meta,
    ) -> String {
        let start = Instant::now();
        let ctx = Self::extract_ctx(&meta, args.analytics.as_ref());
        let project = self.resolve_project(args.project.as_deref()).await;
        let max_depth = args.max_depth.unwrap_or(3);
        let limit = args.limit.unwrap_or(50);
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "impact_analysis", &project, start, &r);
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
            self.analytics_log(&ctx, "impact_analysis", &project, start, &r);
            return r;
        };

        if targets.is_empty() {
            let r = json!({"error": "Target not found"}).to_string();
            self.analytics_log(&ctx, "impact_analysis", &project, start, &r);
            return r;
        }

        // Aggregate impact across all target nodes
        let mut all_direct = Vec::new();
        let mut all_indirect = Vec::new();
        let mut all_files = std::collections::BTreeSet::new();
        let mut seen_ids = std::collections::HashSet::new();

        for t in &targets {
            if let Ok((direct, all, files)) = store.impact_bfs(t.id, max_depth, limit) {
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

        let direct_samples: Vec<Value> = all_direct
            .iter()
            .take(10)
            .map(|n| {
                json!({
                    "name": n.name, "qualified_name": n.qualified_name, "file_path": n.file_path,
                })
            })
            .collect();
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

        let result = json!({
            "project": project, "target": target_info,
            "summary": {
                "direct_dependents": direct_count, "indirect_dependents": indirect_count,
                "affected_files": file_count, "affected_modules": mod_count,
                "cross_project_hits": cross_hits, "risk_level": risk,
            },
            "direct_samples": direct_samples, "indirect_samples": indirect_samples,
            "affected_file_paths": file_paths,
        })
        .to_string();
        self.analytics_log(&ctx, "impact_analysis", &project, start, &result);
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
        let project = self.resolve_project(args.project.as_deref()).await;
        let store = match self.get_store().await {
            Ok(s) => s,
            Err(e) => {
                let r = json!({"error": e.to_string()}).to_string();
                self.analytics_log(&ctx, "explain_index_result", &project, start, &r);
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

        self.analytics_log(&ctx, "explain_index_result", &project, start, &result);
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
        self.analytics_log(&ctx, "get_file_overview", &project, start, &result);
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
        self.analytics_log(&ctx, "find_entrypoints", &project, start, &result);
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
        self.analytics_log(&ctx, "suggest_next_reads", &project, start, &result);
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
        self.analytics_log(&ctx, "trace_data_flow", &project, start, &result);
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
        self.analytics_log(&ctx, "find_tests_for_target", &project, start, &result);
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
        self.analytics_log(&ctx, "suggest_project_links", &project_name, start, &result);
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
        self.analytics_log(&ctx, "find_routes", &project, start, &result);
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
        self.analytics_log(&ctx, "trace_backend_flow", &project, start, &result);
        result
    }
}

// ── Explain helpers in helpers.rs ──────────────────────

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
                 trace_backend_flow to explain full backend request flows (route→controller→service→repository)."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
