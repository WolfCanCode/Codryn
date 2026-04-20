use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use std::collections::{HashSet, VecDeque};

pub struct BackendFlowService;

#[derive(Debug, Serialize)]
pub struct BackendFlowResult {
    pub entry: FlowEntry,
    pub flow: FlowDetail,
    pub summary: FlowSummary,
    pub graph: FlowGraph,
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<FlowAlternative>,
}

#[derive(Debug, Serialize)]
pub struct FlowAlternative {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub score: f64,
}

#[derive(Debug, Serialize)]
pub struct FlowEntry {
    pub method: String,
    pub path: String,
    pub handler: String,
}

#[derive(Debug, Serialize)]
pub struct FlowDetail {
    pub controller: String,
    pub request_dto: Option<String>,
    pub validations: Vec<String>,
    pub service_chain: Vec<FlowChainItem>,
    pub repository_chain: Vec<FlowChainItem>,
    pub response_dto: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowChainItem {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
}

#[derive(Debug, Serialize)]
pub struct FlowSummary {
    pub confidence: f64,
    pub flow_type: String,
}

#[derive(Debug, Serialize)]
pub struct FlowGraph {
    pub nodes: Vec<FlowGraphNode>,
    pub edges: Vec<FlowGraphEdge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowGraphNode {
    pub id: i64,
    pub name: String,
    pub label: String,
    pub layer: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowGraphEdge {
    pub source: i64,
    pub target: i64,
    pub edge_type: String,
}

fn detect_layer(file_path: &str, name: &str) -> &'static str {
    let fp = file_path.to_lowercase();
    let n = name.to_lowercase();
    if fp.contains("lambda")
        || fp.contains("/handlers/")
        || fp.contains("\\handlers\\")
        || fp.contains("serverless")
    {
        return "handler";
    }
    if fp.contains("controller")
        || fp.contains("/api/")
        || n.ends_with("controller")
        || n.ends_with("resource")
    {
        "controller"
    } else if fp.contains("service") || fp.contains("usecase") || n.ends_with("service") {
        "service"
    } else if fp.contains("repo")
        || fp.contains("dao")
        || fp.contains("/store/")
        || n.ends_with("repository")
        || n.ends_with("repo")
    {
        "repository"
    } else if n.ends_with("dto")
        || fp.contains("dto")
        || n.ends_with("request")
        || n.ends_with("response")
    {
        "dto"
    } else {
        "unknown"
    }
}

/// Check node properties for indexed layer, fall back to heuristic.
fn detect_layer_from_props(props_json: Option<&str>, file_path: &str, name: &str) -> &'static str {
    if let Some(props) = props_json {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(props) {
            if let Some(l) = v.get("layer").and_then(|l| l.as_str()) {
                return match l {
                    "controller" => "controller",
                    "handler" => "handler",
                    "service" => "service",
                    "repository" => "repository",
                    "dto" | "entity" | "model" => "dto",
                    "config" => "config",
                    _ => "unknown",
                };
            }
        }
    }
    detect_layer(file_path, name)
}

impl BackendFlowService {
    pub fn trace(
        store: &Store,
        project: &str,
        route_path: Option<&str>,
        handler: Option<&str>,
        http_method: Option<&str>,
        max_depth: i32,
        _include_linked: bool,
    ) -> Result<BackendFlowResult> {
        let max_depth = if max_depth <= 0 { 5 } else { max_depth };
        let mut notes = Vec::new();

        // Step 1: Find the route
        let scope = route_path.or(handler);
        let routes = store.find_routes(project, scope, http_method, 10, false)?;
        if routes.is_empty() {
            return Err(anyhow::anyhow!("No matching routes found"));
        }
        let route = &routes[0];
        let alternatives: Vec<FlowAlternative> = routes
            .iter()
            .skip(1)
            .take(5)
            .map(|r| FlowAlternative {
                method: r.method.clone(),
                path: r.path.clone(),
                handler: r.handler.clone(),
                score: r.score,
            })
            .collect();

        // Step 2: Find the Route node by its dedicated QN.
        // route.route_node_qn is always the Route node's own QN (set by find_routes from the
        // Route label node), regardless of which framework created it.
        let route_node = store
            .find_node_by_qn(project, &route.route_node_qn)?
            .or_else(|| {
                // Fallback: full-text search by path, take first Route label match.
                store
                    .search_nodes(project, &route.path, 10)
                    .ok()?
                    .into_iter()
                    .find(|n| n.label == "Route")
            });
        let route_node =
            route_node.ok_or_else(|| anyhow::anyhow!("Route node not found in graph"))?;

        // Get handler node via HANDLES_ROUTE inbound edge
        let handler_edges = store.get_edges_from_node(route_node.id, "in", 5)?;
        let handler_node_id = handler_edges
            .iter()
            .find(|(_, _, _, _, _, _, et)| et == "HANDLES_ROUTE")
            .map(|(id, _, _, _, _, _, _)| *id);

        // Collect DTOs from route edges
        let route_out = store.get_edges_from_node(route_node.id, "out", 10)?;
        let request_dto = route_out
            .iter()
            .find(|e| e.6 == "ACCEPTS_DTO")
            .map(|e| e.1.clone());
        let response_dto = route_out
            .iter()
            .find(|e| e.6 == "RETURNS_DTO")
            .map(|e| e.1.clone());

        // Step 3: BFS from handler (or route) following CALLS edges
        let start_id = handler_node_id.unwrap_or(route_node.id);
        let mut graph_nodes: Vec<FlowGraphNode> = Vec::new();
        let mut graph_edges: Vec<FlowGraphEdge> = Vec::new();
        let mut services = Vec::new();
        let mut repositories = Vec::new();
        let mut validations = Vec::new();
        let mut controller_name = route.controller.clone();
        let mut visited = HashSet::new();
        let mut queue: VecDeque<(i64, i32)> = VecDeque::new();

        // Add route node to graph
        graph_nodes.push(FlowGraphNode {
            id: route_node.id,
            name: route.handler.clone(),
            label: "Route".into(),
            layer: "route".into(),
            file_path: route.file_path.clone(),
        });

        visited.insert(route_node.id);
        visited.insert(start_id);
        queue.push_back((start_id, 0));

        // Add handler node if different from route
        if start_id != route_node.id {
            if let Some((_, name, _, label, fp, _, _)) = handler_edges
                .iter()
                .find(|(_, _, _, _, _, _, et)| et == "HANDLES_ROUTE")
            {
                let props = store
                    .find_node_by_qn(project, &route.qualified_name)
                    .ok()
                    .flatten()
                    .and_then(|n| n.properties_json);
                let layer = detect_layer_from_props(props.as_deref(), fp, name);
                if layer == "controller" {
                    controller_name = name.clone();
                }
                graph_nodes.push(FlowGraphNode {
                    id: start_id,
                    name: name.clone(),
                    label: label.clone(),
                    layer: layer.into(),
                    file_path: fp.clone(),
                });
                graph_edges.push(FlowGraphEdge {
                    source: route_node.id,
                    target: start_id,
                    edge_type: "HANDLES_ROUTE".into(),
                });
            }
        }

        while let Some((node_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let edges = store
                .get_edges_from_node(node_id, "out", 30)
                .unwrap_or_default();
            for (tgt_id, name, qn, label, fp, _sl, edge_type) in &edges {
                if !matches!(edge_type.as_str(), "CALLS" | "ASYNC_CALLS") {
                    continue;
                }
                let layer = {
                    let node_props = store
                        .find_node_by_qn(project, qn)
                        .ok()
                        .flatten()
                        .and_then(|n| n.properties_json);
                    detect_layer_from_props(node_props.as_deref(), fp, name)
                };
                let item = FlowChainItem {
                    name: name.clone(),
                    qualified_name: qn.clone(),
                    file_path: fp.clone(),
                };

                match layer {
                    "service" => services.push(item),
                    "repository" => repositories.push(item),
                    "controller" if controller_name == route.controller => {
                        controller_name = name.clone();
                    }
                    _ => {}
                }

                // Validation detection
                let nl = name.to_lowercase();
                if nl.contains("valid") || nl.contains("guard") || nl.contains("check") {
                    validations.push(name.clone());
                }
                let props_str = store
                    .find_node_by_qn(project, qn)
                    .ok()
                    .flatten()
                    .and_then(|n| n.properties_json)
                    .unwrap_or_default();
                if props_str.contains("Valid") {
                    validations.push(format!("@Valid on {}", name));
                }

                if !visited.contains(tgt_id) {
                    visited.insert(*tgt_id);
                    graph_nodes.push(FlowGraphNode {
                        id: *tgt_id,
                        name: name.clone(),
                        label: label.clone(),
                        layer: layer.into(),
                        file_path: fp.clone(),
                    });
                    queue.push_back((*tgt_id, depth + 1));
                }
                graph_edges.push(FlowGraphEdge {
                    source: node_id,
                    target: *tgt_id,
                    edge_type: edge_type.clone(),
                });
            }
        }

        // Step 8: Compute confidence and flow_type
        let has_controller = graph_nodes.iter().any(|n| n.layer == "controller");
        let has_handler = graph_nodes.iter().any(|n| n.layer == "handler");
        let has_service = !services.is_empty();
        let has_repo = !repositories.is_empty();

        let mut layers_found = Vec::new();
        if has_controller {
            layers_found.push("controller");
        }
        if has_handler {
            layers_found.push("handler");
        }
        if has_service {
            layers_found.push("service");
        }
        if has_repo {
            layers_found.push("repository");
        }

        let confidence = match layers_found.len() {
            3 => 0.95,
            2 => 0.75,
            1 => 0.55,
            _ => 0.3,
        };
        let flow_type = if layers_found.is_empty() {
            notes.push("Could not detect architectural layers — graph may lack call edges".into());
            "unknown".into()
        } else {
            layers_found.join("-")
        };

        Ok(BackendFlowResult {
            entry: FlowEntry {
                method: route.method.clone(),
                path: route.path.clone(),
                handler: route.handler.clone(),
            },
            flow: FlowDetail {
                controller: controller_name,
                request_dto,
                validations,
                service_chain: services,
                repository_chain: repositories,
                response_dto,
            },
            summary: FlowSummary {
                confidence,
                flow_type,
            },
            graph: FlowGraph {
                nodes: graph_nodes,
                edges: graph_edges,
            },
            notes,
            alternatives,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Edge, FileHash, Node, Project};

    fn setup_store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        // Add file hashes so routes aren't filtered as stale
        s.upsert_file_hash_batch(&[
            FileHash {
                project: "p".into(),
                rel_path: "src/controller/UserController.java".into(),
                sha256: "a".into(),
                mtime_ns: 0,
                size: 1,
            },
            FileHash {
                project: "p".into(),
                rel_path: "src/service/UserService.java".into(),
                sha256: "b".into(),
                mtime_ns: 0,
                size: 1,
            },
            FileHash {
                project: "p".into(),
                rel_path: "src/repository/UserRepo.java".into(),
                sha256: "c".into(),
                mtime_ns: 0,
                size: 1,
            },
        ])
        .unwrap();
        s
    }

    fn insert_node(
        s: &Store,
        name: &str,
        qn: &str,
        label: &str,
        fp: &str,
        props: Option<&str>,
    ) -> i64 {
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: label.into(),
            name: name.into(),
            qualified_name: qn.into(),
            file_path: fp.into(),
            start_line: 1,
            end_line: 10,
            properties_json: props.map(String::from),
        })
        .unwrap()
    }

    fn insert_edge(s: &Store, src: i64, tgt: i64, et: &str) {
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: src,
            target_id: tgt,
            edge_type: et.into(),
            properties_json: None,
        })
        .unwrap();
    }

    #[test]
    fn test_trace_backend_flow_full_chain() {
        let s = setup_store();

        // Route node
        let route_id = insert_node(
            &s,
            "GET /users",
            "p.route.GET./users",
            "Route",
            "src/controller/UserController.java",
            Some(r#"{"http_method":"GET","path":"/users"}"#),
        );

        // Controller method
        let ctrl_id = insert_node(
            &s,
            "getUsers",
            "p.getUsers",
            "Method",
            "src/controller/UserController.java",
            None,
        );
        insert_edge(&s, ctrl_id, route_id, "HANDLES_ROUTE");

        // Service
        let svc_id = insert_node(
            &s,
            "findAllUsers",
            "p.findAllUsers",
            "Method",
            "src/service/UserService.java",
            None,
        );
        insert_edge(&s, ctrl_id, svc_id, "CALLS");

        // Repository
        let repo_id = insert_node(
            &s,
            "findAll",
            "p.findAll",
            "Method",
            "src/repository/UserRepo.java",
            None,
        );
        insert_edge(&s, svc_id, repo_id, "CALLS");

        // DTO edges
        let dto_id = insert_node(
            &s,
            "UserDto",
            "p.UserDto",
            "Class",
            "src/dto/UserDto.java",
            None,
        );
        insert_edge(&s, route_id, dto_id, "RETURNS_DTO");

        let result =
            BackendFlowService::trace(&s, "p", Some("/users"), None, Some("GET"), 5, false)
                .unwrap();

        assert_eq!(result.entry.method, "GET");
        assert_eq!(result.entry.path, "/users");
        assert!(!result.entry.handler.is_empty());
        assert!(
            !result.flow.service_chain.is_empty(),
            "service_chain should not be empty"
        );
        assert!(
            !result.flow.repository_chain.is_empty(),
            "repository_chain should not be empty"
        );
        assert_eq!(result.flow.response_dto.as_deref(), Some("UserDto"));
        assert!(!result.graph.nodes.is_empty());
        assert!(!result.graph.edges.is_empty());
        assert!(result.summary.confidence > 0.5);
        assert!(result.summary.flow_type.contains("controller"));
        assert!(result.summary.flow_type.contains("service"));
        assert!(result.summary.flow_type.contains("repository"));
    }

    #[test]
    fn test_trace_backend_flow_partial() {
        let s = setup_store();
        let route_id = insert_node(
            &s,
            "GET /health",
            "p.route.GET./health",
            "Route",
            "src/controller/UserController.java",
            Some(r#"{"http_method":"GET","path":"/health"}"#),
        );
        let ctrl_id = insert_node(
            &s,
            "healthCheck",
            "p.healthCheck",
            "Method",
            "src/controller/UserController.java",
            None,
        );
        insert_edge(&s, ctrl_id, route_id, "HANDLES_ROUTE");
        // No service or repo — partial flow

        let result =
            BackendFlowService::trace(&s, "p", Some("/health"), None, Some("GET"), 5, false)
                .unwrap();
        assert_eq!(result.entry.path, "/health");
        assert!(result.summary.confidence < 0.95);
    }

    #[test]
    fn test_trace_lambda_style_handler() {
        let s = setup_store();
        s.upsert_file_hash_batch(&[FileHash {
            project: "p".into(),
            rel_path: "src/handlers/api.ts".into(),
            sha256: "d".into(),
            mtime_ns: 0,
            size: 1,
        }])
        .unwrap();

        let route_id = insert_node(
            &s,
            "GET /items",
            "p.lambda.route.GET.items",
            "Route",
            "src/handlers/api.ts",
            Some(r#"{"http_method":"GET","path":"/items","source":"lambda"}"#),
        );
        let handler_id = insert_node(
            &s,
            "handler",
            "p.src.handlers.api.handler",
            "Function",
            "src/handlers/api.ts",
            None,
        );
        insert_edge(&s, handler_id, route_id, "HANDLES_ROUTE");
        let util_id = insert_node(
            &s,
            "parseItems",
            "p.parseItems",
            "Function",
            "src/handlers/util.ts",
            None,
        );
        insert_edge(&s, handler_id, util_id, "CALLS");

        let result =
            BackendFlowService::trace(&s, "p", Some("/items"), None, Some("GET"), 5, false)
                .unwrap();
        assert_eq!(result.entry.path, "/items");
        assert!(result.summary.flow_type.contains("handler"));
    }
}
