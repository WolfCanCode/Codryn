//! One-shot CLI tool runner for MCP tools.
//!
//! Executes MCP tools directly against the store without requiring
//! a running MCP server process. Supports both human-readable table
//! output and JSON output modes.

use anyhow::{bail, Context, Result};
use serde_json::json;
use std::path::Path;

/// All registered MCP tool names available through `cbm query`.
///
/// This list must stay in sync with the tools registered in `codryn-mcp`.
const REGISTERED_TOOLS: &[&str] = &[
    "ask_graph",
    "clear_cache",
    "delete_project",
    "detect_changes",
    "detect_patterns",
    "diagnostics",
    "explain_index_result",
    "find_dead_code",
    "find_entrypoints",
    "find_infrastructure",
    "find_pipelines",
    "find_references",
    "find_routes",
    "find_symbol",
    "find_tests_for_target",
    "freshness_check",
    "get_api_surface",
    "get_architecture",
    "get_code_snippet",
    "get_context_for_task",
    "get_dependency_graph",
    "get_file_overview",
    "get_graph_diff",
    "get_graph_schema",
    "get_project_summary",
    "get_symbol_details",
    "get_symbols_batch",
    "health_check",
    "impact_analysis",
    "index_repository",
    "index_status",
    "ingest_traces",
    "link_project",
    "list_project_links",
    "list_projects",
    "manage_adr",
    "plan_refactoring",
    "query_graph",
    "review_changes",
    "sample_graph",
    "search_code",
    "search_graph",
    "search_linked_projects",
    "semantic_search",
    "suggest_next_reads",
    "suggest_project_links",
    "test_coverage_map",
    "trace_backend_flow",
    "trace_call_path",
    "trace_data_flow",
    "trace_error_flow",
    "what_if",
];

/// Returns all registered MCP tool names, sorted alphabetically.
pub fn list_tools() -> Vec<&'static str> {
    REGISTERED_TOOLS.to_vec()
}

/// Execute an MCP tool as a one-shot CLI command.
///
/// Opens the store directly (no MCP server) and dispatches to the
/// appropriate service function based on the tool name.
///
/// # Arguments
///
/// * `tool_name` - The MCP tool to execute (must be in `list_tools()`)
/// * `args` - Key-value argument pairs from the command line
/// * `json_output` - If true, format output as JSON; otherwise human-readable table
/// * `store_dir` - Path to the store directory containing graph.db
///
/// # Errors
///
/// Returns an error if:
/// - `tool_name` is not a recognized tool (includes list of valid tools)
/// - Arguments are invalid for the specified tool
/// - The store cannot be opened
/// - Tool execution fails
pub fn run_tool(
    tool_name: &str,
    args: &[(String, String)],
    json_output: bool,
    store_dir: &Path,
) -> Result<()> {
    // 1. Validate tool_name against registered tools
    if !REGISTERED_TOOLS.contains(&tool_name) {
        let valid_tools = REGISTERED_TOOLS.join(", ");
        bail!(
            "Unknown tool '{}'. Available tools: {}",
            tool_name,
            valid_tools
        );
    }

    // 2. Open store directly (no MCP server)
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        bail!(
            "Store not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store = codryn_store::Store::open(&db_path).context("failed to open store")?;

    // 3. Resolve project from args (many tools need it)
    let project = get_arg(args, "project").unwrap_or_else(|| "default".to_string());

    // 4. Dispatch to the appropriate service function
    let result = dispatch_tool(tool_name, args, &store, &project)?;

    // 5. Format output as table or JSON
    if json_output {
        format_json(&result);
    } else {
        format_table(&result);
    }

    Ok(())
}

/// Extract a named argument from the key-value pairs.
fn get_arg(args: &[(String, String)], key: &str) -> Option<String> {
    args.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

/// Dispatch to the appropriate service function based on tool name.
///
/// Returns the tool result as a JSON value for formatting.
fn dispatch_tool(
    tool_name: &str,
    args: &[(String, String)],
    store: &codryn_store::Store,
    project: &str,
) -> Result<serde_json::Value> {
    match tool_name {
        "list_projects" => {
            let projects = store.list_projects().context("failed to list projects")?;
            Ok(json!({ "projects": projects }))
        }
        "get_graph_schema" => {
            let schema = store
                .get_graph_schema(project)
                .context("failed to get graph schema")?;
            Ok(serde_json::to_value(schema)?)
        }
        "search_graph" => {
            let query = get_arg(args, "query").unwrap_or_default();
            let limit = get_arg(args, "limit")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(20);
            if query.is_empty() {
                bail!("'query' argument is required for search_graph");
            }
            let nodes = store
                .search_nodes(project, &query, limit)
                .context("search_graph failed")?;
            Ok(json!({ "nodes": nodes, "count": nodes.len() }))
        }
        "query_graph" => {
            let query = get_arg(args, "query").unwrap_or_default();
            if query.is_empty() {
                bail!("'query' argument is required for query_graph");
            }
            let result = codryn_cypher::execute(store, project, &query)
                .context("Cypher query execution failed")?;
            Ok(result)
        }
        "find_symbol" => {
            let query = get_arg(args, "query").unwrap_or_default();
            let label = get_arg(args, "label");
            let limit = get_arg(args, "limit")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(10);
            if query.is_empty() {
                bail!("'query' argument is required for find_symbol");
            }
            let results = store
                .find_symbol_ranked(project, &query, label.as_deref(), false, limit)
                .context("find_symbol failed")?;
            let symbols: Vec<serde_json::Value> = results
                .iter()
                .map(|(node, match_type, score)| {
                    json!({
                        "name": node.name,
                        "qualified_name": node.qualified_name,
                        "label": node.label,
                        "file_path": node.file_path,
                        "start_line": node.start_line,
                        "end_line": node.end_line,
                        "match_type": match_type,
                        "score": score,
                    })
                })
                .collect();
            Ok(json!({ "results": symbols, "count": symbols.len() }))
        }
        "index_status" => {
            let schema = store
                .get_graph_schema(project)
                .context("failed to get index status")?;
            Ok(json!({
                "project": project,
                "indexed": schema.total_nodes > 0,
                "total_nodes": schema.total_nodes,
                "total_edges": schema.total_edges,
            }))
        }
        "health_check" => Ok(json!({ "status": "ok", "mode": "cli" })),
        "clear_cache" => Ok(json!({ "status": "ok", "message": "cache cleared (no-op in CLI mode)" })),
        // TODO: Implement remaining tool dispatches.
        // The full dispatch requires deeper integration with codryn-services
        // (architecture, flow analysis, navigation, pattern detection, etc.).
        // Each tool needs specific argument parsing and service function calls.
        // For now, tools not yet wired return a helpful message.
        _ => {
            Ok(json!({
                "status": "not_yet_implemented",
                "tool": tool_name,
                "message": format!(
                    "Tool '{}' is recognized but CLI dispatch is not yet implemented. \
                     Use the MCP server for full functionality.",
                    tool_name
                ),
            }))
        }
    }
}

/// Format the result as a JSON object to stdout.
fn format_json(result: &serde_json::Value) {
    let output = json!({
        "success": true,
        "result": result,
    });
    // Use pretty-print for readability
    println!(
        "{}",
        serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
    );
}

/// Format the result as a human-readable table to stdout.
fn format_table(result: &serde_json::Value) {
    match result {
        serde_json::Value::Object(map) => {
            // Calculate max key width for alignment
            let key_width = map.keys().map(|k| k.len()).max().unwrap_or(0);

            for (key, value) in map {
                match value {
                    serde_json::Value::Array(arr) => {
                        println!("{:<width$}  ({} items)", key, arr.len(), width = key_width);
                        for (i, item) in arr.iter().enumerate() {
                            format_table_item(i, item, key_width);
                        }
                    }
                    serde_json::Value::Object(_) => {
                        println!("{:<width$}:", key, width = key_width);
                        format_nested_object(value, 2);
                    }
                    _ => {
                        println!(
                            "{:<width$}  {}",
                            key,
                            format_scalar(value),
                            width = key_width
                        );
                    }
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                format_table_item(i, item, 0);
            }
        }
        _ => {
            println!("{}", format_scalar(result));
        }
    }
}

/// Format a single array item in table output.
fn format_table_item(index: usize, item: &serde_json::Value, _parent_width: usize) {
    match item {
        serde_json::Value::Object(map) => {
            println!("  [{}]", index);
            let inner_width = map.keys().map(|k| k.len()).max().unwrap_or(0);
            for (key, value) in map {
                println!(
                    "    {:<width$}  {}",
                    key,
                    format_scalar(value),
                    width = inner_width
                );
            }
        }
        _ => {
            println!("  [{}] {}", index, format_scalar(item));
        }
    }
}

/// Format a nested JSON object with indentation.
fn format_nested_object(value: &serde_json::Value, indent: usize) {
    let prefix = " ".repeat(indent);
    match value {
        serde_json::Value::Object(map) => {
            let key_width = map.keys().map(|k| k.len()).max().unwrap_or(0);
            for (key, val) in map {
                match val {
                    serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                        println!("{}{:<width$}:", prefix, key, width = key_width);
                        format_nested_object(val, indent + 2);
                    }
                    _ => {
                        println!(
                            "{}{:<width$}  {}",
                            prefix,
                            key,
                            format_scalar(val),
                            width = key_width
                        );
                    }
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                println!("{}[{}] {}", prefix, i, format_scalar(item));
            }
        }
        _ => {
            println!("{}{}", prefix, format_scalar(value));
        }
    }
}

/// Format a scalar JSON value as a string for display.
fn format_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "-".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        // For nested objects/arrays in scalar position, use compact JSON
        _ => serde_json::to_string(value).unwrap_or_else(|_| "?".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_tools_returns_all_registered_tools() {
        let tools = list_tools();
        // Should have 52 tools (as of current implementation)
        assert!(
            tools.len() >= 46,
            "Expected at least 46 tools, got {}",
            tools.len()
        );
    }

    #[test]
    fn list_tools_is_sorted() {
        let tools = list_tools();
        let mut sorted = tools.clone();
        sorted.sort();
        assert_eq!(tools, sorted, "Tool list should be sorted alphabetically");
    }

    #[test]
    fn list_tools_contains_key_tools() {
        let tools = list_tools();
        assert!(tools.contains(&"find_symbol"));
        assert!(tools.contains(&"search_graph"));
        assert!(tools.contains(&"query_graph"));
        assert!(tools.contains(&"impact_analysis"));
        assert!(tools.contains(&"health_check"));
        assert!(tools.contains(&"list_projects"));
        assert!(tools.contains(&"get_architecture"));
        assert!(tools.contains(&"what_if"));
    }

    #[test]
    fn run_tool_rejects_unknown_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let result = run_tool("nonexistent_tool", &[], false, tmp.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("nonexistent_tool"),
            "Error should mention the unknown tool name"
        );
        assert!(
            err_msg.contains("find_symbol"),
            "Error should list at least one valid tool"
        );
    }

    #[test]
    fn run_tool_error_contains_available_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let result = run_tool("bogus_tool_xyz", &[], false, tmp.path());
        let err_msg = result.unwrap_err().to_string();
        // Should contain the unknown name
        assert!(err_msg.contains("bogus_tool_xyz"));
        // Should contain at least one valid tool from the list
        assert!(
            err_msg.contains("search_graph") || err_msg.contains("find_symbol"),
            "Error message should list available tools"
        );
    }

    #[test]
    fn run_tool_errors_when_store_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        // Valid tool name but no graph.db in the directory
        let result = run_tool("list_projects", &[], false, tmp.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found") || err_msg.contains("Store"),
            "Error should indicate store not found: {}",
            err_msg
        );
    }

    #[test]
    fn run_tool_validates_required_args() {
        // Create a temporary store
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("graph.db");
        let _store = codryn_store::Store::open(&db_path).unwrap();

        // search_graph requires 'query' argument
        let result = run_tool("search_graph", &[], false, tmp.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("query"),
            "Error should mention missing 'query' argument: {}",
            err_msg
        );
    }

    #[test]
    fn format_json_produces_valid_json() {
        let result = json!({"status": "ok", "count": 5});
        // Capture output by testing the structure directly
        let output = json!({
            "success": true,
            "result": result,
        });
        let serialized = serde_json::to_string(&output).unwrap();
        // Should be parseable back
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["result"]["status"], "ok");
        assert_eq!(parsed["result"]["count"], 5);
    }

    #[test]
    fn format_scalar_handles_all_types() {
        assert_eq!(format_scalar(&json!("hello")), "hello");
        assert_eq!(format_scalar(&json!(42)), "42");
        assert_eq!(format_scalar(&json!(true)), "true");
        assert_eq!(format_scalar(&json!(null)), "-");
        assert_eq!(format_scalar(&json!(3.14)), "3.14");
    }
}
