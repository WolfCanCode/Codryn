use anyhow::Result;
use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use codryn_store::Store;
use rust_embed::Embed;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Embed)]
#[folder = "../../ui/dist/"]
#[prefix = ""]
struct Assets;

pub struct UiConfig {
    pub enabled: bool,
    pub port: u16,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 9749,
        }
    }
}

struct AppState {
    store_path: PathBuf,
}

pub async fn start_server(store_path: &Path, port: u16) -> Result<()> {
    let state = Arc::new(AppState {
        store_path: store_path.to_owned(),
    });

    let app = Router::new()
        .route(
            "/rpc",
            post({
                let state = state.clone();
                move |body| handle_rpc(state, body)
            }),
        )
        .route(
            "/api/layout",
            get({
                let state = state.clone();
                move |query| handle_layout(state, query)
            }),
        )
        .route("/api/doctor", get(handle_doctor))
        .route("/api/browse", get(handle_browse))
        .route(
            "/api/storage",
            get({
                let state = state.clone();
                move || handle_storage(state)
            }),
        )
        .route(
            "/api/analytics",
            get({
                let state = state.clone();
                move || handle_analytics(state)
            }),
        )
        .route("/api/install", post(handle_install))
        .route("/api/uninstall", post(handle_uninstall))
        .route(
            "/api/backend-flow",
            get({
                let state = state.clone();
                move |query| handle_backend_flow(state, query)
            }),
        )
        .route(
            "/api/frontend-flow",
            get({
                let state = state.clone();
                move |query| handle_frontend_flow(state, query)
            }),
        )
        .route(
            "/api/logo",
            get({
                let state = state.clone();
                move |query| handle_logo(state, query)
            }),
        )
        .route(
            "/api/languages",
            get({
                let state = state.clone();
                move |query| handle_languages(state, query)
            }),
        )
        .fallback(serve_static);

    let addr = format!("127.0.0.1:{}", port);
    tracing::info!(addr = %addr, "UI server starting");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_rpc(state: Arc<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let method = body["method"].as_str().unwrap_or("");
    let params = &body["params"];
    let id = &body["id"];

    if method != "tools/call" {
        return Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "method not found" }
        }));
    }

    let tool_name = params["name"].as_str().unwrap_or("");
    let args = &params["arguments"];
    let project_arg = args["project"].as_str().unwrap_or("").to_owned();
    let start = std::time::Instant::now();

    let store = match open_store(&state.store_path) {
        Ok(s) => s,
        Err(e) => {
            return Json(json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32000, "message": e.to_string() }
            }));
        }
    };

    let result = match tool_name {
        "list_projects" => {
            let projects = store.list_projects().unwrap_or_default();
            json!({ "projects": projects })
        }
        "get_graph_schema" => {
            let project = args["project"].as_str().unwrap_or("default");
            store
                .get_graph_schema(project)
                .map(|s| serde_json::to_value(s).unwrap())
                .unwrap_or(json!({}))
        }
        "search_graph" => {
            let project = args["project"].as_str().unwrap_or("default");
            let query = args["query"].as_str().unwrap_or("");
            let limit = args["limit"].as_i64().unwrap_or(20) as i32;
            let nodes = store
                .search_nodes(project, query, limit)
                .unwrap_or_default();
            json!({ "nodes": nodes, "count": nodes.len() })
        }
        "query_graph" => {
            let project = args["project"].as_str().unwrap_or("default");
            let query = args["query"].as_str().unwrap_or("");
            codryn_cypher::execute(&store, project, query)
                .unwrap_or(json!({ "error": "query failed" }))
        }
        "delete_project" => {
            let project = args["project"].as_str().unwrap_or("");
            match store.delete_project(project) {
                Ok(()) => json!({ "status": "deleted", "project": project }),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "index_status" => {
            let project = args["project"].as_str().unwrap_or("default");
            match store.get_graph_schema(project) {
                Ok(schema) => {
                    json!({ "project": project, "indexed": schema.total_nodes > 0, "total_nodes": schema.total_nodes, "total_edges": schema.total_edges })
                }
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "index_repository" => {
            let path = args["path"].as_str().unwrap_or("");
            let mode = match args["mode"].as_str() {
                Some("fast") => codryn_pipeline::IndexMode::Fast,
                _ => codryn_pipeline::IndexMode::Full,
            };
            let repo = std::path::PathBuf::from(path);
            let pipeline = codryn_pipeline::Pipeline::new(&repo, &state.store_path, mode);
            match pipeline.run() {
                Ok(()) => json!({ "status": "ok", "project": pipeline.project_name() }),
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "link_project" => {
            let project = args["project"].as_str().unwrap_or("");
            let target = args["target_project"].as_str().unwrap_or("");
            let action = args["action"].as_str().unwrap_or("");
            let projects = store.list_projects().unwrap_or_default();
            let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
            if !names.contains(&project) {
                json!({ "error": format!("Project '{}' not found", project) })
            } else if !names.contains(&target) {
                json!({ "error": format!("Project '{}' not found", target) })
            } else {
                match action {
                    "link" => store.link_projects(project, target).map(|_| json!({ "status": "linked", "project": project, "target_project": target })).unwrap_or_else(|e| json!({ "error": e.to_string() })),
                    "unlink" => store.unlink_projects(project, target).map(|_| json!({ "status": "unlinked", "project": project, "target_project": target })).unwrap_or_else(|e| json!({ "error": e.to_string() })),
                    _ => json!({ "error": "action must be 'link' or 'unlink'" }),
                }
            }
        }
        "list_project_links" => {
            let project = args["project"].as_str().unwrap_or("default");
            match store.get_linked_projects(project) {
                Ok(links) => {
                    let targets: Vec<&str> =
                        links.iter().map(|l| l.target_project.as_str()).collect();
                    json!({ "project": project, "linked_projects": targets, "count": links.len() })
                }
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        "search_linked_projects" => {
            let project = args["project"].as_str().unwrap_or("default");
            let query = args["query"].as_str().unwrap_or("");
            let label = args["label"].as_str();
            let limit = args["limit"].as_i64().unwrap_or(20) as i32;
            let links = store.get_linked_projects(project).unwrap_or_default();
            let mut results = Vec::new();
            for link in &links {
                let remaining = limit - results.len() as i32;
                if remaining <= 0 {
                    break;
                }
                let nodes = store
                    .search_nodes_filtered(&link.target_project, query, label, remaining)
                    .unwrap_or_default();
                for n in nodes {
                    results.push(json!({ "source_project": link.target_project, "name": n.name, "qualified_name": n.qualified_name, "label": n.label, "file_path": n.file_path }));
                }
            }
            json!({ "from_project": project, "results": results, "count": results.len() })
        }
        "trace_call_path" => {
            let project = args["project"].as_str().unwrap_or("default");
            let source = args["source"].as_str().unwrap_or("");
            let target = args["target"].as_str().unwrap_or("");
            let max_depth = args["max_depth"].as_i64().unwrap_or(5) as i32;
            let tgt = if target.is_empty() {
                None
            } else {
                Some(target)
            };
            match store.trace_calls(project, source, tgt, max_depth) {
                Ok(steps) => {
                    let path: Vec<_> = steps
                        .iter()
                        .map(|(src, tgt, sf, tf)| {
                            json!({
                                "caller": src, "callee": tgt, "caller_file": sf, "callee_file": tf,
                            })
                        })
                        .collect();
                    json!({ "path": path, "steps": path.len() })
                }
                Err(e) => json!({ "error": e.to_string() }),
            }
        }
        _ => json!({ "error": format!("unknown tool: {}", tool_name) }),
    };

    // Log the tool call for analytics
    let duration_ms = start.elapsed().as_millis() as i64;
    let result_str = result.to_string();
    let success = !result_str.contains("\"error\"");
    let response_bytes = result_str.len() as i64;
    if let Ok(log_store) = open_store(&state.store_path) {
        let _ = log_store.log_tool_call(
            tool_name,
            &project_arg,
            "ui",
            duration_ms,
            success,
            "",
            "",
            0,
            0,
            response_bytes,
        );
    }

    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": result.to_string() }]
        }
    }))
}

#[derive(Deserialize)]
struct LayoutQuery {
    project: String,
    max_nodes: Option<usize>,
}

async fn handle_layout(state: Arc<AppState>, Query(q): Query<LayoutQuery>) -> Json<Value> {
    let store = match open_store(&state.store_path) {
        Ok(s) => s,
        Err(_) => return Json(json!({ "nodes": [], "edges": [], "total_nodes": 0 })),
    };

    let max = q.max_nodes.unwrap_or(5000);
    let nodes = store
        .search_nodes(&q.project, "%", max as i32)
        .unwrap_or_default();

    let node_ids: std::collections::HashSet<i64> = nodes.iter().map(|n| n.id).collect();

    let layout_nodes: Vec<Value> = nodes
        .iter()
        .map(|n| {
            json!({
                "id": n.id,
                "label": n.label,
                "name": n.name,
                "file_path": n.file_path,
                "size": 5,
                "color": label_color(&n.label),
            })
        })
        .collect();

    // Fetch edges and filter to only include edges between visible nodes
    let all_edges = store
        .get_edges(&q.project, (max * 3) as i32)
        .unwrap_or_default();
    let layout_edges: Vec<Value> = all_edges
        .iter()
        .filter(|e| node_ids.contains(&e.source_id) && node_ids.contains(&e.target_id))
        .map(|e| json!({ "source": e.source_id, "target": e.target_id, "type": e.edge_type }))
        .collect();

    Json(json!({
        "nodes": layout_nodes,
        "edges": layout_edges,
        "total_nodes": nodes.len(),
    }))
}

fn label_color(label: &str) -> &'static str {
    match label {
        "Function" => "#1976d2",
        "Class" => "#e64a19",
        "Method" => "#388e3c",
        "Module" => "#7b1fa2",
        "File" => "#546e7a",
        "Folder" => "#37474f",
        "Interface" => "#f9a825",
        "Project" => "#c62828",
        _ => "#78909c",
    }
}

async fn handle_doctor() -> Json<Value> {
    let report = codryn_cli::doctor::run_doctor();
    Json(serde_json::to_value(report).unwrap_or(json!({})))
}

async fn handle_analytics(state: Arc<AppState>) -> Json<Value> {
    match open_store(&state.store_path) {
        Ok(s) => match s.get_tool_analytics(100) {
            Ok(a) => Json(serde_json::to_value(a).unwrap_or(json!({}))),
            Err(e) => Json(json!({ "error": e.to_string() })),
        },
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

async fn handle_storage(state: Arc<AppState>) -> Json<Value> {
    let db_path = state.store_path.join("graph.db");
    let db_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    let bin_bytes = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);

    fn human(bytes: u64) -> String {
        if bytes >= 1_073_741_824 {
            format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
        } else if bytes >= 1_048_576 {
            format!("{:.1} MB", bytes as f64 / 1_048_576.0)
        } else if bytes >= 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{} B", bytes)
        }
    }

    Json(json!({
        "graph_db_bytes": db_bytes,
        "graph_db_human": human(db_bytes),
        "binary_bytes": bin_bytes,
        "binary_human": human(bin_bytes),
    }))
}

async fn handle_install() -> Json<Value> {
    let binary = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("codryn"));
    match codryn_cli::install::install(&binary, false) {
        Ok(configured) => Json(json!({ "status": "ok", "configured": configured })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

async fn handle_uninstall() -> Json<Value> {
    match codryn_cli::install::uninstall(false) {
        Ok(removed) => Json(json!({ "status": "ok", "removed": removed })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

#[derive(Deserialize)]
struct BackendFlowQuery {
    project: String,
    route_path: Option<String>,
    handler: Option<String>,
    http_method: Option<String>,
    list: Option<String>,
}

async fn handle_backend_flow(
    state: Arc<AppState>,
    Query(q): Query<BackendFlowQuery>,
) -> Json<Value> {
    let store = match open_store(&state.store_path) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    // list=true → return routes list only
    if q.list.as_deref() == Some("true") {
        return match store.find_routes(&q.project, None, None, 100, false) {
            Ok(routes) => Json(json!({"routes": routes, "count": routes.len()})),
            Err(e) => Json(json!({"error": e.to_string()})),
        };
    }

    match codryn_services::backend_flow::BackendFlowService::trace(
        &store,
        &q.project,
        q.route_path.as_deref(),
        q.handler.as_deref(),
        q.http_method.as_deref(),
        5,
        false,
    ) {
        Ok(result) => {
            let nodes: Vec<Value> = result.graph.nodes.iter().map(|n| {
                let color = match n.layer.as_str() {
                    "route" => "#c62828",
                    "controller" => "#1976d2",
                    "service" => "#388e3c",
                    "repository" => "#e64a19",
                    "dto" => "#f9a825",
                    _ => "#78909c",
                };
                json!({"id": n.id, "name": n.name, "label": n.label, "layer": n.layer, "file_path": n.file_path, "color": color, "size": 8})
            }).collect();
            let edges: Vec<Value> = result
                .graph
                .edges
                .iter()
                .map(|e| json!({"source": e.source, "target": e.target, "type": e.edge_type}))
                .collect();
            Json(json!({
                "nodes": nodes,
                "edges": edges,
                "entry": result.entry,
                "flow": result.flow,
                "summary": result.summary,
                "notes": result.notes,
            }))
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[derive(Deserialize)]
struct FrontendFlowQuery {
    project: String,
    component: Option<String>,
    list: Option<String>,
}

async fn handle_frontend_flow(
    state: Arc<AppState>,
    Query(q): Query<FrontendFlowQuery>,
) -> Json<Value> {
    let store = match open_store(&state.store_path) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    // list=true → return components list
    if q.list.as_deref() == Some("true") {
        let nodes = store.get_all_nodes(&q.project).unwrap_or_default();
        let components: Vec<Value> = nodes.iter()
            .filter(|n| {
                let props = n.properties_json.as_deref().unwrap_or("{}");
                let v = serde_json::from_str::<serde_json::Value>(props).ok().unwrap_or_default();
                let dec = v.get("decorator").and_then(|d| d.as_str());
                let fw = v.get("framework").and_then(|d| d.as_str());
                matches!(dec, Some("Component") | Some("Directive"))
                    || matches!(fw, Some("react") | Some("solid") | Some("vue"))
            })
            .map(|n| {
                let props: serde_json::Value = serde_json::from_str(n.properties_json.as_deref().unwrap_or("{}")).unwrap_or_default();
                let selector = props.get("selector").and_then(|s| s.as_str()).unwrap_or(
                    props.get("framework").and_then(|s| s.as_str()).unwrap_or(""),
                );
                json!({"name": n.name, "selector": selector, "file_path": n.file_path, "qualified_name": n.qualified_name})
            })
            .collect();
        return Json(json!({"components": components, "count": components.len()}));
    }

    // Trace component flow
    let comp_name = q.component.as_deref().unwrap_or("");
    let node = store
        .find_symbol_ranked(&q.project, comp_name, Some("Class"), false, 3)
        .unwrap_or_default()
        .into_iter()
        .find(|(n, _, _)| {
            let props = n.properties_json.as_deref().unwrap_or("{}");
            let v: serde_json::Value = serde_json::from_str(props).unwrap_or_default();
            v.get("decorator").is_some()
                || matches!(
                    v.get("framework").and_then(|x| x.as_str()),
                    Some("react") | Some("solid") | Some("vue")
                )
        })
        .or_else(|| {
            store
                .find_symbol_ranked(&q.project, comp_name, Some("Function"), false, 5)
                .unwrap_or_default()
                .into_iter()
                .find(|(n, _, _)| {
                    let props = n.properties_json.as_deref().unwrap_or("{}");
                    let v: serde_json::Value = serde_json::from_str(props).unwrap_or_default();
                    matches!(
                        v.get("framework").and_then(|x| x.as_str()),
                        Some("react") | Some("solid") | Some("vue")
                    )
                })
        })
        .map(|(n, _, _)| n);
    let node = match node {
        Some(n) => n,
        None => return Json(json!({"error": format!("Component not found: {comp_name}")})),
    };

    let mut graph_nodes: Vec<Value> = Vec::new();
    let mut graph_edges: Vec<Value> = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    let layer_color = |layer: &str| match layer {
        "component" => "#c62828",
        "service" => "#1976d2",
        "directive" => "#7b1fa2",
        "pipe" => "#f9a825",
        "module" => "#00838f",
        "vue" | "react" | "solid" => "#2e7d32",
        _ => "#78909c",
    };

    let node_layer = |n: &codryn_store::Node| -> String {
        let v: serde_json::Value =
            serde_json::from_str(n.properties_json.as_deref().unwrap_or("{}")).unwrap_or_default();
        v.get("layer")
            .and_then(|l| l.as_str())
            .map(String::from)
            .or_else(|| v.get("framework").and_then(|l| l.as_str()).map(String::from))
            .unwrap_or_else(|| "unknown".into())
    };

    let layer = node_layer(&node);
    graph_nodes.push(json!({"id": node.id, "name": node.name, "label": node.label, "layer": &layer, "file_path": node.file_path, "color": layer_color(&layer), "size": 10}));
    visited.insert(node.id);
    queue.push_back((node.id, 0));

    while let Some((nid, depth)) = queue.pop_front() {
        if depth >= 4 {
            continue;
        }
        let edges = store
            .get_edges_from_node(nid, "out", 30)
            .unwrap_or_default();
        for (tid, name, _qn, label, fp, _sl, etype) in &edges {
            if !matches!(etype.as_str(), "RENDERS" | "INJECTS" | "SELECTS" | "CALLS") {
                continue;
            }
            if !visited.insert(*tid) {
                graph_edges.push(json!({"source": nid, "target": tid, "type": etype}));
                continue;
            }
            let child_layer = store
                .find_node_by_qn(&q.project, _qn)
                .ok()
                .flatten()
                .map(|n| node_layer(&n))
                .unwrap_or_else(|| {
                    if fp.contains("service") || name.ends_with("Service") {
                        "service".into()
                    } else if fp.contains("component") {
                        "component".into()
                    } else {
                        "unknown".into()
                    }
                });
            graph_nodes.push(json!({"id": tid, "name": name, "label": label, "layer": &child_layer, "file_path": fp, "color": layer_color(&child_layer), "size": 8}));
            graph_edges.push(json!({"source": nid, "target": tid, "type": etype}));
            queue.push_back((*tid, depth + 1));
        }
    }

    let services: Vec<&Value> = graph_nodes
        .iter()
        .filter(|n| n["layer"] == "service")
        .collect();
    let components: Vec<&Value> = graph_nodes
        .iter()
        .filter(|n| n["layer"] == "component")
        .collect();
    let confidence = if graph_nodes.len() > 3 {
        0.9
    } else if graph_nodes.len() > 1 {
        0.7
    } else {
        0.4
    };

    Json(json!({
        "nodes": graph_nodes,
        "edges": graph_edges,
        "summary": {
            "flow_type": "component",
            "confidence": confidence,
            "renders": components.len().saturating_sub(1),
            "injects": services.len(),
        },
    }))
}

#[derive(Deserialize)]
struct LogoQuery {
    project: String,
}

const LOGO_CANDIDATES: &[&str] = &[
    "logo.png",
    "logo.svg",
    "logo.jpg",
    "logo.webp",
    "public/logo.png",
    "public/logo.svg",
    "public/logo.jpg",
    "public/logo.webp",
    "public/favicon.svg",
    "public/favicon.png",
    "public/favicon.ico",
    "assets/logo.png",
    "assets/logo.svg",
    "assets/logo.jpg",
    "src/assets/logo.png",
    "src/assets/logo.svg",
    "src/assets/logo.jpg",
    "src/favicon.ico",
    "src/favicon.svg",
    "src/favicon.png",
    "favicon.svg",
    "favicon.png",
    "favicon.ico",
    "static/logo.png",
    "static/logo.svg",
    "public/icon.png",
    "public/icon.svg",
    "icon.png",
    "icon.svg",
    "dist/favicon.ico",
    "dist/favicon.png",
    "dist/favicon.svg",
];

const LOGO_DIRS: &[&str] = &[
    "",
    "public",
    "src",
    "assets",
    "src/assets",
    "static",
    "dist",
    "dist/img",
];
const LOGO_NAMES: &[&str] = &["logo", "favicon", "icon", "icon-512", "icon-192"];
const LOGO_EXTS: &[&str] = &[".svg", ".png", ".webp", ".jpg", ".ico"];

async fn handle_logo(state: Arc<AppState>, Query(q): Query<LogoQuery>) -> Response {
    let store = match open_store(&state.store_path) {
        Ok(s) => s,
        Err(_) => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    let projects = store.list_projects().unwrap_or_default();
    let root = match projects.iter().find(|p| p.name == q.project) {
        Some(p) => p.root_path.clone(),
        None => return (StatusCode::NOT_FOUND, "project not found").into_response(),
    };

    // Try exact candidates first
    for candidate in LOGO_CANDIDATES {
        let path = Path::new(&root).join(candidate);
        if path.is_file() {
            return serve_logo_file(&path);
        }
    }

    // Then try combinations of dirs × names × extensions
    for dir in LOGO_DIRS {
        for name in LOGO_NAMES {
            for ext in LOGO_EXTS {
                let path = Path::new(&root).join(dir).join(format!("{}{}", name, ext));
                if path.is_file() {
                    return serve_logo_file(&path);
                }
            }
        }
    }

    // Last resort: find any image in src/assets/
    for dir in &["src/assets", "assets", "public"] {
        let search_dir = Path::new(&root).join(dir);
        if let Ok(entries) = std::fs::read_dir(&search_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                        if matches!(ext, "svg" | "png" | "jpg" | "webp" | "ico") {
                            return serve_logo_file(&p);
                        }
                    }
                }
            }
        }
    }

    (StatusCode::NOT_FOUND, "no logo found").into_response()
}

fn serve_logo_file(path: &Path) -> Response {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime.to_string()),
                    (header::CACHE_CONTROL, "public, max-age=3600".into()),
                ],
                bytes,
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "read error").into_response(),
    }
}

#[derive(Deserialize)]
struct BrowseQuery {
    path: Option<String>,
}

async fn handle_browse(Query(q): Query<BrowseQuery>) -> Json<Value> {
    let base = q
        .path
        .unwrap_or_else(|| codryn_foundation::platform::home_dir().unwrap_or_else(|| "/".into()));
    let base = if base == "~" {
        codryn_foundation::platform::home_dir().unwrap_or_else(|| "/".into())
    } else {
        base
    };
    let path = Path::new(&base);
    if !path.is_dir() {
        return Json(json!({ "error": "not a directory", "path": base }));
    }
    let mut dirs: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !name.starts_with('.') {
                        // Full paths so the client opens subfolders correctly (bare names resolve
                        // relative to the server process cwd).
                        let child = path.join(&name);
                        dirs.push(child.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    dirs.sort();
    let parent = path.parent().map(|p| p.to_string_lossy().to_string());
    Json(json!({ "path": base, "parent": parent, "dirs": dirs }))
}

async fn serve_static(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref())],
                content.data.to_vec(),
            )
                .into_response()
        }
        None => {
            // SPA fallback: serve index.html for unknown routes
            match Assets::get("index.html") {
                Some(content) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html")],
                    content.data.to_vec(),
                )
                    .into_response(),
                None => (StatusCode::NOT_FOUND, "not found").into_response(),
            }
        }
    }
}

#[derive(Deserialize)]
struct LangQuery {
    project: String,
}

async fn handle_languages(state: Arc<AppState>, Query(q): Query<LangQuery>) -> Json<Value> {
    let store = match open_store(&state.store_path) {
        Ok(s) => s,
        Err(_) => return Json(json!({"languages": [], "frameworks": [], "libraries": []})),
    };
    let langs = store.get_project_languages(&q.project).unwrap_or_default();
    let edges = store.get_graph_schema(&q.project).ok();
    let edge_set: std::collections::HashSet<String> = edges
        .iter()
        .flat_map(|s| s.edge_types.iter().map(|e| e.edge_type.clone()))
        .collect();
    let items: Vec<Value> = langs
        .iter()
        .map(|(l, c)| json!({"language": l, "count": c}))
        .collect();
    // --- Accurate framework detection ---
    // Route nodes carry a `source` property set by each routing pass:
    //   "nextjs" | "lambda" | "express"  (spring routes have no source field)
    let route_sources = store.get_route_sources(&q.project).unwrap_or_default();
    let has_spring = store.has_spring_routes(&q.project).unwrap_or(false);
    // Node `framework` property is set by jsx_framework and vue_sfc passes.
    let node_frameworks = store.get_node_frameworks(&q.project).unwrap_or_default();

    let mut frameworks: Vec<String> = Vec::new();
    // Spring Boot: route nodes with no source AND Java/Kotlin files present
    let has_java_kotlin = langs.iter().any(|(l, _)| l == "Java" || l == "Kotlin");
    if has_spring && has_java_kotlin {
        frameworks.push("Spring Boot".into());
    }
    if route_sources.iter().any(|s| s == "nextjs") {
        frameworks.push("Next.js".into());
    }
    if route_sources.iter().any(|s| s == "lambda") {
        frameworks.push("AWS Lambda".into());
    }
    if route_sources.iter().any(|s| s == "serverless") {
        frameworks.push("Serverless".into());
    }
    if route_sources.iter().any(|s| s == "express") {
        frameworks.push("Express".into());
    }
    // Angular: SELECTS (router-outlet) and INJECTS (DI) are Angular-specific;
    // React/Vue also emit RENDERS so we do NOT use RENDERS here.
    if edge_set.contains("SELECTS") || edge_set.contains("INJECTS") {
        frameworks.push("Angular".into());
    }

    // Libraries: inferred from node `framework` property tags.
    let mut libraries: Vec<String> = Vec::new();
    if node_frameworks.iter().any(|f| f == "react") {
        libraries.push("React".into());
    }
    if node_frameworks.iter().any(|f| f == "solid") {
        libraries.push("Solid.js".into());
    }
    if node_frameworks.iter().any(|f| f == "vue") {
        libraries.push("Vue".into());
    }
    Json(json!({
        "languages": items,
        "frameworks": frameworks,
        "libraries": libraries,
    }))
}

fn open_store(path: &Path) -> Result<Store> {
    if path.to_string_lossy() == ":memory:" {
        Store::open_in_memory()
    } else {
        Store::open(&path.join("graph.db"))
    }
}
