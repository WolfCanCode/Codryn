use anyhow::Result;
use codryn_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Maximum number of route entries to include in the generated OpenAPI spec.
const MAX_ROUTES: i32 = 500;

/// Represents a single endpoint change in the API surface diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointChange {
    pub method: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_handler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_dto: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_dto: Option<String>,
}

/// Result of comparing the current API surface against the most recent snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDiffResult {
    pub snapshot_id: i64,
    pub snapshot_timestamp: String,
    pub added: Vec<EndpointChange>,
    pub removed: Vec<EndpointChange>,
    pub modified: Vec<EndpointChange>,
    pub total_changes: usize,
}

/// A compact key for identifying an endpoint by method + path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EndpointKey {
    method: String,
    path: String,
}

/// Generate an OpenAPI 3.0 JSON document from indexed Route nodes for a project.
///
/// The generated spec includes:
/// - `openapi`: version string ("3.0.3")
/// - `info`: title derived from project name, version "0.0.0"
/// - `paths`: one entry per route with method, operationId from handler name
/// - `components/schemas`: DTO references discovered via ACCEPTS_DTO/RETURNS_DTO edges
///
/// Routes without linked DTOs omit `requestBody` and `responses.content.schema`.
/// Limited to 500 route entries maximum.
pub fn generate_openapi(store: &Store, project: &str) -> Result<Value> {
    let routes = store.find_routes(project, None, None, MAX_ROUTES, false)?;

    let mut paths: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut schemas: serde_json::Map<String, Value> = serde_json::Map::new();

    for route in &routes {
        let method = route.method.to_lowercase();
        if method.is_empty() {
            continue;
        }

        let path = if route.path.is_empty() {
            "/".to_string()
        } else {
            route.path.clone()
        };

        // Build operationId from handler name
        let operation_id = sanitize_operation_id(&route.handler);

        // Build the operation object
        let mut operation: serde_json::Map<String, Value> = serde_json::Map::new();
        operation.insert("operationId".to_string(), json!(operation_id));

        // Add requestBody if request DTO is linked
        if let Some(ref dto_name) = route.request_dto {
            let schema_ref = format!("#/components/schemas/{}", dto_name);
            operation.insert(
                "requestBody".to_string(),
                json!({
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": { "$ref": schema_ref }
                        }
                    }
                }),
            );
            // Register the DTO in schemas if not already present
            if !schemas.contains_key(dto_name) {
                schemas.insert(dto_name.clone(), json!({ "type": "object" }));
            }
        }

        // Add responses if response DTO is linked
        if let Some(ref dto_name) = route.response_dto {
            let schema_ref = format!("#/components/schemas/{}", dto_name);
            operation.insert(
                "responses".to_string(),
                json!({
                    "200": {
                        "description": "Successful response",
                        "content": {
                            "application/json": {
                                "schema": { "$ref": schema_ref }
                            }
                        }
                    }
                }),
            );
            // Register the DTO in schemas if not already present
            if !schemas.contains_key(dto_name) {
                schemas.insert(dto_name.clone(), json!({ "type": "object" }));
            }
        } else if !operation.contains_key("responses") {
            // OpenAPI requires at least one response; add a minimal one without schema
            operation.insert(
                "responses".to_string(),
                json!({
                    "200": {
                        "description": "Successful response"
                    }
                }),
            );
        }

        // Insert into paths, merging methods for the same path
        let path_item = paths.entry(path).or_insert_with(|| json!({}));
        if let Some(obj) = path_item.as_object_mut() {
            obj.insert(method, json!(operation));
        }
    }

    // Build the final OpenAPI document
    let mut doc = json!({
        "openapi": "3.0.3",
        "info": {
            "title": format!("{} API", project),
            "version": "0.0.0"
        },
        "paths": paths
    });

    // Only include components/schemas if there are DTOs
    if !schemas.is_empty() {
        doc.as_object_mut()
            .unwrap()
            .insert("components".to_string(), json!({ "schemas": schemas }));
    }

    Ok(doc)
}

/// Sanitize a handler name into a valid operationId.
/// Replaces non-alphanumeric characters (except underscores) with underscores,
/// and strips leading/trailing underscores.
fn sanitize_operation_id(handler: &str) -> String {
    let sanitized: String = handler
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    sanitized.trim_matches('_').to_string()
}

/// Compare the current set of Route nodes against the most recently stored index snapshot.
///
/// Returns a structured diff listing endpoints categorized as:
/// - **added**: path+method exists now but not in the snapshot
/// - **removed**: path+method exists in the snapshot but not now
/// - **modified**: same path+method but changed handler, request DTO, or response DTO
///
/// Returns an error when no prior snapshot with route data exists (Requirement 25.5).
pub fn diff_api_surface(store: &Store, project: &str) -> Result<ApiDiffResult> {
    use codryn_store::SnapshotRoute;
    use std::collections::HashMap;

    // Get the most recent snapshot that has route data
    let snapshot = store.get_latest_route_snapshot(project)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no prior snapshot with route data exists for project '{}'; \
             run index_repository first to create a baseline snapshot",
            project
        )
    })?;

    let snapshot_routes = snapshot.routes.unwrap_or_default();

    // Get current routes (include_deleted=true to avoid stale-file filtering
    // which would incorrectly exclude routes that haven't been indexed with file hashes)
    let current_routes = store.find_routes(project, None, None, MAX_ROUTES, true)?;

    // Build lookup maps keyed by (method, path)
    let mut snapshot_map: HashMap<EndpointKey, &SnapshotRoute> = HashMap::new();
    for route in &snapshot_routes {
        let key = EndpointKey {
            method: route.method.to_uppercase(),
            path: route.path.clone(),
        };
        snapshot_map.insert(key, route);
    }

    let mut current_map: HashMap<EndpointKey, &codryn_store::RouteInfo> = HashMap::new();
    for route in &current_routes {
        let key = EndpointKey {
            method: route.method.to_uppercase(),
            path: route.path.clone(),
        };
        current_map.insert(key, route);
    }

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();

    // Find added and modified endpoints
    for (key, current) in &current_map {
        if let Some(prev) = snapshot_map.get(key) {
            // Endpoint exists in both — check for modifications
            let handler_changed = current.handler != prev.handler;
            let request_dto_changed = current.request_dto != prev.request_dto;
            let response_dto_changed = current.response_dto != prev.response_dto;

            if handler_changed || request_dto_changed || response_dto_changed {
                modified.push(EndpointChange {
                    method: key.method.clone(),
                    path: key.path.clone(),
                    handler: Some(current.handler.clone()),
                    previous_handler: if handler_changed {
                        Some(prev.handler.clone())
                    } else {
                        None
                    },
                    request_dto: current.request_dto.clone(),
                    response_dto: current.response_dto.clone(),
                });
            }
        } else {
            // Endpoint exists now but not in snapshot — added
            added.push(EndpointChange {
                method: key.method.clone(),
                path: key.path.clone(),
                handler: Some(current.handler.clone()),
                previous_handler: None,
                request_dto: current.request_dto.clone(),
                response_dto: current.response_dto.clone(),
            });
        }
    }

    // Find removed endpoints (in snapshot but not in current)
    for (key, prev) in &snapshot_map {
        if !current_map.contains_key(key) {
            removed.push(EndpointChange {
                method: key.method.clone(),
                path: key.path.clone(),
                handler: Some(prev.handler.clone()),
                previous_handler: None,
                request_dto: prev.request_dto.clone(),
                response_dto: prev.response_dto.clone(),
            });
        }
    }

    // Sort for deterministic output
    added.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    removed.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    modified.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));

    let total_changes = added.len() + removed.len() + modified.len();

    Ok(ApiDiffResult {
        snapshot_id: snapshot.id,
        snapshot_timestamp: snapshot.timestamp,
        added,
        removed,
        modified,
        total_changes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_operation_id() {
        assert_eq!(sanitize_operation_id("getUsers"), "getUsers");
        assert_eq!(sanitize_operation_id("get_users"), "get_users");
        assert_eq!(
            sanitize_operation_id("com.example.getUsers"),
            "com_example_getUsers"
        );
        assert_eq!(sanitize_operation_id(""), "");
        assert_eq!(sanitize_operation_id("...handler..."), "handler");
    }

    #[test]
    fn test_generate_openapi_empty_project() {
        // Create an in-memory store for testing
        let store = Store::open_in_memory().expect("Failed to create in-memory store");
        let result = generate_openapi(&store, "test-project").unwrap();

        assert_eq!(result["openapi"], "3.0.3");
        assert_eq!(result["info"]["title"], "test-project API");
        assert_eq!(result["info"]["version"], "0.0.0");
        // paths should be empty object
        assert!(result["paths"].as_object().unwrap().is_empty());
        // No components when no DTOs
        assert!(result.get("components").is_none());
    }

    #[test]
    fn test_diff_api_surface_no_snapshot_returns_error() {
        let store = Store::open_in_memory().expect("Failed to create in-memory store");
        store
            .upsert_project(&codryn_store::Project {
                name: "test-project".to_string(),
                indexed_at: "2025-01-01".to_string(),
                root_path: "/tmp".to_string(),
            })
            .unwrap();

        let result = diff_api_surface(&store, "test-project");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("no prior snapshot"));
    }

    #[test]
    fn test_diff_api_surface_no_changes() {
        let store = Store::open_in_memory().expect("Failed to create in-memory store");
        store
            .upsert_project(&codryn_store::Project {
                name: "test-project".to_string(),
                indexed_at: "2025-01-01".to_string(),
                root_path: "/tmp".to_string(),
            })
            .unwrap();

        // Insert a Route node
        let route_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Route".to_string(),
                name: "GET /users".to_string(),
                qualified_name: "test-project.GET./users".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(r#"{"http_method":"GET","path":"/users"}"#.to_string()),
            })
            .unwrap();

        // Insert a handler node and HANDLES_ROUTE edge
        let handler_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Function".to_string(),
                name: "get_users".to_string(),
                qualified_name: "test-project.get_users".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "test-project".to_string(),
                source_id: handler_id,
                target_id: route_id,
                edge_type: "HANDLES_ROUTE".to_string(),
                properties_json: None,
            })
            .unwrap();

        // Record a snapshot (captures current routes)
        store.record_snapshot("test-project", None).unwrap();

        // Now diff — routes haven't changed
        let result = diff_api_surface(&store, "test-project").unwrap();
        assert_eq!(result.added.len(), 0);
        assert_eq!(result.removed.len(), 0);
        assert_eq!(result.modified.len(), 0);
        assert_eq!(result.total_changes, 0);
    }

    #[test]
    fn test_diff_api_surface_detects_added_endpoint() {
        let store = Store::open_in_memory().expect("Failed to create in-memory store");
        store
            .upsert_project(&codryn_store::Project {
                name: "test-project".to_string(),
                indexed_at: "2025-01-01".to_string(),
                root_path: "/tmp".to_string(),
            })
            .unwrap();

        // Insert initial route
        let route_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Route".to_string(),
                name: "GET /users".to_string(),
                qualified_name: "test-project.GET./users".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(r#"{"http_method":"GET","path":"/users"}"#.to_string()),
            })
            .unwrap();

        let handler_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Function".to_string(),
                name: "get_users".to_string(),
                qualified_name: "test-project.get_users".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "test-project".to_string(),
                source_id: handler_id,
                target_id: route_id,
                edge_type: "HANDLES_ROUTE".to_string(),
                properties_json: None,
            })
            .unwrap();

        // Record snapshot with just the one route
        store.record_snapshot("test-project", None).unwrap();

        // Add a new route
        let new_route_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Route".to_string(),
                name: "POST /users".to_string(),
                qualified_name: "test-project.POST./users".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 15,
                end_line: 25,
                properties_json: Some(r#"{"http_method":"POST","path":"/users"}"#.to_string()),
            })
            .unwrap();

        let new_handler_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Function".to_string(),
                name: "create_user".to_string(),
                qualified_name: "test-project.create_user".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 15,
                end_line: 25,
                properties_json: None,
            })
            .unwrap();

        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "test-project".to_string(),
                source_id: new_handler_id,
                target_id: new_route_id,
                edge_type: "HANDLES_ROUTE".to_string(),
                properties_json: None,
            })
            .unwrap();

        // Diff should show the new route as added
        let result = diff_api_surface(&store, "test-project").unwrap();
        assert_eq!(result.added.len(), 1);
        assert_eq!(result.removed.len(), 0);
        assert_eq!(result.modified.len(), 0);
        assert_eq!(result.added[0].method, "POST");
        assert_eq!(result.added[0].path, "/users");
        assert_eq!(result.added[0].handler, Some("create_user".to_string()));
    }

    #[test]
    fn test_diff_api_surface_detects_removed_endpoint() {
        let store = Store::open_in_memory().expect("Failed to create in-memory store");
        store
            .upsert_project(&codryn_store::Project {
                name: "test-project".to_string(),
                indexed_at: "2025-01-01".to_string(),
                root_path: "/tmp".to_string(),
            })
            .unwrap();

        // Insert two routes
        let route1_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Route".to_string(),
                name: "GET /users".to_string(),
                qualified_name: "test-project.GET./users".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(r#"{"http_method":"GET","path":"/users"}"#.to_string()),
            })
            .unwrap();

        let handler1_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Function".to_string(),
                name: "get_users".to_string(),
                qualified_name: "test-project.get_users".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "test-project".to_string(),
                source_id: handler1_id,
                target_id: route1_id,
                edge_type: "HANDLES_ROUTE".to_string(),
                properties_json: None,
            })
            .unwrap();

        let route2_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Route".to_string(),
                name: "DELETE /users".to_string(),
                qualified_name: "test-project.DELETE./users".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 20,
                end_line: 30,
                properties_json: Some(r#"{"http_method":"DELETE","path":"/users"}"#.to_string()),
            })
            .unwrap();

        let handler2_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Function".to_string(),
                name: "delete_user".to_string(),
                qualified_name: "test-project.delete_user".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 20,
                end_line: 30,
                properties_json: None,
            })
            .unwrap();

        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "test-project".to_string(),
                source_id: handler2_id,
                target_id: route2_id,
                edge_type: "HANDLES_ROUTE".to_string(),
                properties_json: None,
            })
            .unwrap();

        // Record snapshot with both routes
        store.record_snapshot("test-project", None).unwrap();

        // Remove the DELETE route
        store
            .conn()
            .execute(
                "DELETE FROM nodes WHERE id = ?1",
                rusqlite::params![route2_id],
            )
            .unwrap();

        // Diff should show the DELETE route as removed
        let result = diff_api_surface(&store, "test-project").unwrap();
        assert_eq!(result.added.len(), 0);
        assert_eq!(result.removed.len(), 1);
        assert_eq!(result.removed[0].method, "DELETE");
        assert_eq!(result.removed[0].path, "/users");
    }

    #[test]
    fn test_diff_api_surface_detects_modified_handler() {
        let store = Store::open_in_memory().expect("Failed to create in-memory store");
        store
            .upsert_project(&codryn_store::Project {
                name: "test-project".to_string(),
                indexed_at: "2025-01-01".to_string(),
                root_path: "/tmp".to_string(),
            })
            .unwrap();

        // Insert a route with handler "get_users_v1"
        let route_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Route".to_string(),
                name: "GET /users".to_string(),
                qualified_name: "test-project.GET./users".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(r#"{"http_method":"GET","path":"/users"}"#.to_string()),
            })
            .unwrap();

        let handler_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Function".to_string(),
                name: "get_users_v1".to_string(),
                qualified_name: "test-project.get_users_v1".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "test-project".to_string(),
                source_id: handler_id,
                target_id: route_id,
                edge_type: "HANDLES_ROUTE".to_string(),
                properties_json: None,
            })
            .unwrap();

        // Record snapshot
        store.record_snapshot("test-project", None).unwrap();

        // Change the handler: remove old edge, add new handler + edge
        store
            .conn()
            .execute(
                "DELETE FROM edges WHERE source_id = ?1 AND target_id = ?2",
                rusqlite::params![handler_id, route_id],
            )
            .unwrap();

        let new_handler_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "test-project".to_string(),
                label: "Function".to_string(),
                name: "get_users_v2".to_string(),
                qualified_name: "test-project.get_users_v2".to_string(),
                file_path: "src/routes.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "test-project".to_string(),
                source_id: new_handler_id,
                target_id: route_id,
                edge_type: "HANDLES_ROUTE".to_string(),
                properties_json: None,
            })
            .unwrap();

        // Diff should show the route as modified
        let result = diff_api_surface(&store, "test-project").unwrap();
        assert_eq!(result.added.len(), 0);
        assert_eq!(result.removed.len(), 0);
        assert_eq!(result.modified.len(), 1);
        assert_eq!(result.modified[0].method, "GET");
        assert_eq!(result.modified[0].path, "/users");
        assert_eq!(result.modified[0].handler, Some("get_users_v2".to_string()));
        assert_eq!(
            result.modified[0].previous_handler,
            Some("get_users_v1".to_string())
        );
    }
}
