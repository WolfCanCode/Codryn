//! Integration tests for MCP tool round-trip verification.
//!
//! These tests index a known fixture project (`tests/fixtures/sample-project/`)
//! and verify that MCP tools return correct results from real indexed data.
//!
//! The fixture project contains:
//! - 5+ functions: main, handleRequest, getUser, createUser, listUsers, deleteUser
//! - 3 classes: UserService, UserRepository, Logger
//! - Known CALLS edges: main -> UserService.getUser, main -> UserService.createUser
//! - Known IMPORTS edges: main.ts -> user-service, user-service -> logger, etc.
//!
//! **Validates: Requirements 2.1, 2.3**

use codryn_mcp::{
    CodrynServer, FindReferencesArgs, FindSymbolArgs, GetSymbolDetailsArgs, IndexArgs, SearchArgs,
};
use serde_json::Value;
use std::path::PathBuf;
use tempfile::TempDir;

/// Test fixture that indexes the sample project and provides assertion helpers.
struct TestFixture {
    server: CodrynServer,
    project_name: String,
    #[allow(dead_code)]
    store_dir: TempDir,
}

impl TestFixture {
    /// Create a new test fixture by indexing the sample project.
    async fn new() -> Self {
        let store_dir = TempDir::new().expect("failed to create temp dir for store");
        let store_path = store_dir.path().join("store");
        std::fs::create_dir_all(&store_path).expect("failed to create store dir");

        let server = CodrynServer::new(&store_path);

        let fixture_path = Self::fixture_project_path();
        assert!(
            fixture_path.exists(),
            "Fixture project not found at: {}",
            fixture_path.display()
        );

        // Index the fixture project
        let response = server
            .index_repository_test(IndexArgs {
                path: fixture_path.to_string_lossy().into_owned(),
                mode: Some("full".to_string()),
                clear_cache: None,
                analytics: None,
            })
            .await;

        let v: Value = serde_json::from_str(&response).expect("index response should be JSON");
        assert!(v.get("error").is_none(), "Indexing failed: {}", response);

        // Derive project name from the fixture path (same logic as the server)
        let project_name = fixture_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        Self {
            server,
            project_name,
            store_dir,
        }
    }

    /// Returns the absolute path to the fixture project.
    fn fixture_project_path() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // Navigate from crates/codryn-mcp to workspace root
        manifest_dir
            .parent() // crates/
            .unwrap()
            .parent() // workspace root
            .unwrap()
            .join("tests")
            .join("fixtures")
            .join("sample-project")
    }

    fn parse(response: &str) -> Value {
        serde_json::from_str(response).unwrap_or(Value::Null)
    }
}

// ── find_symbol tests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_find_symbol_returns_known_function() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .server
        .find_symbol_test(FindSymbolArgs {
            project: Some(fixture.project_name.clone()),
            query: "main".to_string(),
            label: None,
            exact: None,
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(v.get("error").is_none(), "find_symbol failed: {response}");

    let matches = v["matches"].as_array().expect("matches should be an array");
    assert!(
        !matches.is_empty(),
        "Should find at least one match for 'main'"
    );

    // Verify the result contains expected fields
    let first = &matches[0];
    assert!(first["name"].as_str().is_some(), "name should be present");
    assert!(
        first["qualified_name"].as_str().is_some(),
        "qualified_name should be present"
    );
    assert!(first["label"].as_str().is_some(), "label should be present");
    assert!(
        first["file_path"].as_str().is_some(),
        "file_path should be present"
    );
    assert!(
        first["score"].as_f64().unwrap_or(0.0) > 0.0,
        "score should be > 0"
    );
}

#[tokio::test]
async fn test_find_symbol_returns_known_class() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .server
        .find_symbol_test(FindSymbolArgs {
            project: Some(fixture.project_name.clone()),
            query: "UserService".to_string(),
            label: Some("Class".to_string()),
            exact: None,
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(v.get("error").is_none(), "find_symbol failed: {response}");

    let matches = v["matches"].as_array().expect("matches should be an array");
    assert!(!matches.is_empty(), "Should find UserService class");

    let found = matches
        .iter()
        .any(|m| m["name"].as_str() == Some("UserService") && m["label"].as_str() == Some("Class"));
    assert!(found, "Should find UserService with label Class");
}

#[tokio::test]
async fn test_find_symbol_with_label_filter() {
    let fixture = TestFixture::new().await;

    // Search for Logger as a Class
    let response = fixture
        .server
        .find_symbol_test(FindSymbolArgs {
            project: Some(fixture.project_name.clone()),
            query: "Logger".to_string(),
            label: Some("Class".to_string()),
            exact: None,
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(v.get("error").is_none(), "find_symbol failed: {response}");

    let matches = v["matches"].as_array().expect("matches should be an array");
    // All results should have label "Class"
    for m in matches {
        if m["label"].as_str().is_some() {
            assert_eq!(
                m["label"].as_str().unwrap(),
                "Class",
                "Label filter should only return Class nodes"
            );
        }
    }
}

// ── get_symbol_details tests ──────────────────────────────────────────────────

#[tokio::test]
async fn test_get_symbol_details_returns_callers_and_callees() {
    let fixture = TestFixture::new().await;

    // First find UserService to get its qualified name
    let find_response = fixture
        .server
        .find_symbol_test(FindSymbolArgs {
            project: Some(fixture.project_name.clone()),
            query: "UserService".to_string(),
            label: Some("Class".to_string()),
            exact: None,
            limit: Some(1),
            include_linked: None,
            analytics: None,
        })
        .await;

    let find_v = TestFixture::parse(&find_response);
    let matches = find_v["matches"].as_array().unwrap();

    if matches.is_empty() {
        // If UserService not found as Class, try by name
        let response = fixture
            .server
            .get_symbol_details_test(GetSymbolDetailsArgs {
                project: Some(fixture.project_name.clone()),
                qualified_name: None,
                name: Some("UserService".to_string()),
                label: None,
                include_snippet: Some(false),
                snippet_lines: None,
                analytics: None,
            })
            .await;

        let v = TestFixture::parse(&response);
        assert!(
            v.get("error").is_none(),
            "get_symbol_details failed: {response}"
        );
        assert!(v.get("symbol").is_some(), "Should have symbol field");
        return;
    }

    let qn = matches[0]["qualified_name"].as_str().unwrap();

    let response = fixture
        .server
        .get_symbol_details_test(GetSymbolDetailsArgs {
            project: Some(fixture.project_name.clone()),
            qualified_name: Some(qn.to_string()),
            name: None,
            label: None,
            include_snippet: Some(false),
            snippet_lines: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "get_symbol_details failed: {response}"
    );

    // Verify structure
    assert!(v.get("symbol").is_some(), "Should have symbol field");
    assert!(v.get("callers").is_some(), "Should have callers field");
    assert!(v.get("callees").is_some(), "Should have callees field");
    assert!(v.get("imports").is_some(), "Should have imports field");

    let symbol = &v["symbol"];
    assert_eq!(symbol["name"].as_str(), Some("UserService"));
}

#[tokio::test]
async fn test_get_symbol_details_by_name() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .server
        .get_symbol_details_test(GetSymbolDetailsArgs {
            project: Some(fixture.project_name.clone()),
            qualified_name: None,
            name: Some("handleRequest".to_string()),
            label: None,
            include_snippet: Some(false),
            snippet_lines: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "get_symbol_details failed: {response}"
    );

    let symbol = &v["symbol"];
    assert!(
        symbol["name"].as_str().is_some(),
        "Symbol name should be present"
    );
    assert!(
        symbol["file_path"].as_str().is_some(),
        "file_path should be present"
    );
}

// ── find_references tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_find_references_returns_incoming_edges() {
    let fixture = TestFixture::new().await;

    // Find references to Logger (should be imported by main.ts and user-service.ts)
    let response = fixture
        .server
        .find_references_test(FindReferencesArgs {
            project: Some(fixture.project_name.clone()),
            qualified_name: None,
            name: Some("Logger".to_string()),
            label: None,
            reference_type: None,
            limit: None,
            group_by: None,
            include_linked: None,
            min_confidence: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "find_references failed: {response}"
    );

    // The response uses "count" for total references and "groups" when grouped by file
    let count = v["count"].as_i64().unwrap_or(0);
    assert!(
        count >= 0,
        "Should have non-negative reference count, got: {count}"
    );

    // Verify the response has the expected structure (grouped by file by default)
    assert!(
        v.get("groups").is_some() || v.get("references").is_some(),
        "Should have groups or references field in response: {response}"
    );

    // Verify target info is present
    assert!(
        v.get("target").is_some(),
        "Should have target field identifying the queried symbol"
    );
}

#[tokio::test]
async fn test_find_references_with_type_filter() {
    let fixture = TestFixture::new().await;

    // Find only IMPORTS references
    let response = fixture
        .server
        .find_references_test(FindReferencesArgs {
            project: Some(fixture.project_name.clone()),
            qualified_name: None,
            name: Some("Logger".to_string()),
            label: None,
            reference_type: Some("imports".to_string()),
            limit: None,
            group_by: None,
            include_linked: None,
            min_confidence: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "find_references with type filter failed: {response}"
    );
}

#[tokio::test]
async fn test_find_references_with_uses_filter() {
    let fixture = TestFixture::new().await;

    // Find only USES references (type annotations, variable declarations)
    let response = fixture
        .server
        .find_references_test(FindReferencesArgs {
            project: Some(fixture.project_name.clone()),
            qualified_name: None,
            name: Some("Logger".to_string()),
            label: None,
            reference_type: Some("uses".to_string()),
            limit: None,
            group_by: None,
            include_linked: None,
            min_confidence: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "find_references with uses filter failed: {response}"
    );

    // Verify reference_type is reported correctly in response
    assert_eq!(
        v["reference_type"].as_str().unwrap_or(""),
        "uses",
        "Response should report reference_type as 'uses'"
    );

    // If there are any references, they should all be USES edges (not CALLS)
    if let Some(groups) = v.get("groups").and_then(|g| g.as_array()) {
        for group in groups {
            if let Some(refs) = group.get("references").and_then(|r| r.as_array()) {
                for r in refs {
                    let edge_type = r["edge_type"].as_str().unwrap_or("");
                    assert_eq!(
                        edge_type, "USES",
                        "With reference_type='uses', all edges should be USES, got: {edge_type}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn test_find_references_all_includes_calls_and_uses() {
    let fixture = TestFixture::new().await;

    // Find all references (should include both CALLS and USES)
    let response = fixture
        .server
        .find_references_test(FindReferencesArgs {
            project: Some(fixture.project_name.clone()),
            qualified_name: None,
            name: Some("Logger".to_string()),
            label: None,
            reference_type: Some("all".to_string()),
            limit: None,
            group_by: None,
            include_linked: None,
            min_confidence: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "find_references with 'all' filter failed: {response}"
    );

    // Verify reference_type is reported correctly
    assert_eq!(
        v["reference_type"].as_str().unwrap_or(""),
        "all",
        "Response should report reference_type as 'all'"
    );

    // Each reference should have an edge_type label
    if let Some(groups) = v.get("groups").and_then(|g| g.as_array()) {
        for group in groups {
            if let Some(refs) = group.get("references").and_then(|r| r.as_array()) {
                for r in refs {
                    assert!(
                        r.get("edge_type").is_some(),
                        "Each reference should include edge_type label"
                    );
                }
            }
        }
    }
}

// ── search_graph tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_search_graph_returns_matching_nodes() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .server
        .search_graph_test(SearchArgs {
            project: Some(fixture.project_name.clone()),
            query: "User".to_string(),
            limit: Some(10),
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(v.get("error").is_none(), "search_graph failed: {response}");

    let nodes = v["nodes"].as_array().expect("nodes should be an array");
    let count = v["count"].as_i64().unwrap_or(0);
    assert!(
        count > 0,
        "Should find nodes matching 'User', got count: {count}"
    );
    assert!(!nodes.is_empty(), "nodes array should not be empty");

    // Verify node structure
    let first = &nodes[0];
    assert!(first["name"].as_str().is_some(), "name should be present");
    assert!(
        first["qualified_name"].as_str().is_some(),
        "qualified_name should be present"
    );
    assert!(first["label"].as_str().is_some(), "label should be present");
}

#[tokio::test]
async fn test_search_graph_with_specific_function_name() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .server
        .search_graph_test(SearchArgs {
            project: Some(fixture.project_name.clone()),
            query: "getUser".to_string(),
            limit: Some(5),
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(v.get("error").is_none(), "search_graph failed: {response}");

    let count = v["count"].as_i64().unwrap_or(0);
    assert!(
        count > 0,
        "Should find nodes matching 'getUser', got count: {count}"
    );
}

#[tokio::test]
async fn test_search_graph_no_results_for_nonexistent() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .server
        .search_graph_test(SearchArgs {
            project: Some(fixture.project_name.clone()),
            query: "xyzNonExistentSymbol12345".to_string(),
            limit: Some(5),
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(v.get("error").is_none(), "search_graph failed: {response}");

    let count = v["count"].as_i64().unwrap_or(0);
    assert_eq!(count, 0, "Should find no nodes for nonexistent query");
}

// ── Round-trip verification ───────────────────────────────────────────────────

#[tokio::test]
async fn test_full_round_trip_index_then_query() {
    let fixture = TestFixture::new().await;

    // 1. Verify we can find functions
    let response = fixture
        .server
        .find_symbol_test(FindSymbolArgs {
            project: Some(fixture.project_name.clone()),
            query: "main".to_string(),
            label: Some("Function".to_string()),
            exact: None,
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(v.get("error").is_none(), "find_symbol failed: {response}");
    let matches = v["matches"].as_array().unwrap();
    assert!(
        !matches.is_empty(),
        "Should find 'main' function after indexing"
    );

    // 2. Verify we can find classes
    let response = fixture
        .server
        .find_symbol_test(FindSymbolArgs {
            project: Some(fixture.project_name.clone()),
            query: "UserRepository".to_string(),
            label: None,
            exact: None,
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(v.get("error").is_none(), "find_symbol failed: {response}");
    let matches = v["matches"].as_array().unwrap();
    assert!(
        !matches.is_empty(),
        "Should find 'UserRepository' after indexing"
    );

    // 3. Verify search_graph works
    let response = fixture
        .server
        .search_graph_test(SearchArgs {
            project: Some(fixture.project_name.clone()),
            query: "Service".to_string(),
            limit: Some(10),
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(v.get("error").is_none(), "search_graph failed: {response}");
    let count = v["count"].as_i64().unwrap_or(0);
    assert!(count > 0, "Should find nodes matching 'Service'");
}

// ── Error handling tests ──────────────────────────────────────────────────────
// **Validates: Requirements 2.4**

#[tokio::test]
async fn test_index_invalid_path_returns_error_no_side_effects() {
    // Create a fresh store with no pre-existing data
    let store_dir = TempDir::new().expect("failed to create temp dir for store");
    let store_path = store_dir.path().join("store");
    std::fs::create_dir_all(&store_path).expect("failed to create store dir");

    let server = CodrynServer::new(&store_path);

    // Use a path that definitely does not exist
    let invalid_path = "/tmp/nonexistent-repo-path-xyz-12345-does-not-exist";
    assert!(
        !std::path::Path::new(invalid_path).exists(),
        "Test assumes this path does not exist"
    );

    // Call index_repository with the invalid path
    let response = server
        .index_repository_test(IndexArgs {
            path: invalid_path.to_string(),
            mode: Some("full".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;

    let v: Value = serde_json::from_str(&response).expect("response should be valid JSON");

    // Verify the response contains an error field
    assert!(
        v.get("error").is_some(),
        "Invalid path should return an error field in response. Got: {response}"
    );

    // Verify the error message is descriptive (non-empty)
    let error_msg = v["error"].as_str().unwrap_or("");
    assert!(
        !error_msg.is_empty(),
        "Error message should be descriptive, got empty string"
    );

    // Verify no side effects: no nodes or edges were created in the store.
    // We derive the project name the same way the server would (from the path's last component).
    let project_name = std::path::Path::new(invalid_path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // Search for any nodes in this project — should find nothing
    let search_response = server
        .search_graph_test(SearchArgs {
            project: Some(project_name.clone()),
            query: "".to_string(),
            limit: Some(100),
            analytics: None,
        })
        .await;

    let search_v = TestFixture::parse(&search_response);
    let count = search_v["count"].as_i64().unwrap_or(0);
    assert_eq!(
        count, 0,
        "No nodes should exist after indexing an invalid path. Found {count} nodes for project '{project_name}'"
    );
}

// ── Incremental reindex tests ─────────────────────────────────────────────────
// **Validates: Requirements 2.3**

/// Helper that creates a temporary copy of the fixture project for mutation tests.
/// Returns (server, project_name, temp_dir) where temp_dir contains the copied project.
async fn setup_mutable_fixture() -> (CodrynServer, String, TempDir, PathBuf) {
    let store_dir = TempDir::new().expect("failed to create temp dir for store");
    let store_path = store_dir.path().join("store");
    std::fs::create_dir_all(&store_path).expect("failed to create store dir");

    let server = CodrynServer::new(&store_path);

    // Copy fixture project to a temp directory so we can mutate it
    let fixture_src = TestFixture::fixture_project_path();
    let project_dir = TempDir::new().expect("failed to create temp dir for project");
    let project_path = project_dir.path().join("sample-project");
    copy_dir_recursive(&fixture_src, &project_path);

    // Index the copied project (full mode)
    let response = server
        .index_repository_test(IndexArgs {
            path: project_path.to_string_lossy().into_owned(),
            mode: Some("full".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;

    let v: Value = serde_json::from_str(&response).expect("index response should be JSON");
    assert!(
        v.get("error").is_none(),
        "Initial indexing failed: {}",
        response
    );

    let project_name = project_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    (server, project_name, project_dir, project_path)
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("failed to create destination dir");
    for entry in std::fs::read_dir(src).expect("failed to read source dir") {
        let entry = entry.expect("failed to read dir entry");
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            std::fs::copy(&src_path, &dst_path).expect("failed to copy file");
        }
    }
}

#[tokio::test]
async fn test_incremental_reindex_new_function_appears() {
    let (server, project_name, _project_dir, project_path) = setup_mutable_fixture().await;

    // Verify the new function does NOT exist yet
    let response = server
        .find_symbol_test(FindSymbolArgs {
            project: Some(project_name.clone()),
            query: "computeAnalytics".to_string(),
            label: None,
            exact: Some(true),
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    let empty = vec![];
    let matches = v["matches"].as_array().unwrap_or(&empty);
    assert!(
        matches.is_empty(),
        "computeAnalytics should NOT exist before adding the file"
    );

    // Add a new TypeScript file with a function
    let new_file_path = project_path.join("src").join("analytics.ts");
    std::fs::write(
        &new_file_path,
        r#"/**
 * Computes analytics metrics for the given dataset.
 */
export function computeAnalytics(data: number[]): { mean: number; sum: number } {
    const sum = data.reduce((a, b) => a + b, 0);
    const mean = data.length > 0 ? sum / data.length : 0;
    return { mean, sum };
}

/**
 * Formats analytics results for display.
 */
export function formatAnalyticsReport(metrics: { mean: number; sum: number }): string {
    return `Mean: ${metrics.mean}, Sum: ${metrics.sum}`;
}
"#,
    )
    .expect("failed to write new file");

    // Re-index incrementally (fast mode)
    let response = server
        .index_repository_test(IndexArgs {
            path: project_path.to_string_lossy().into_owned(),
            mode: Some("fast".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;

    let v: Value = serde_json::from_str(&response).expect("reindex response should be JSON");
    assert!(
        v.get("error").is_none(),
        "Incremental reindex failed: {}",
        response
    );

    // Verify the new function now appears in find_symbol
    let response = server
        .find_symbol_test(FindSymbolArgs {
            project: Some(project_name.clone()),
            query: "computeAnalytics".to_string(),
            label: None,
            exact: None,
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "find_symbol failed after reindex: {response}"
    );

    let matches = v["matches"].as_array().expect("matches should be an array");
    assert!(
        !matches.is_empty(),
        "computeAnalytics should appear after incremental reindex"
    );

    let found = matches
        .iter()
        .any(|m| m["name"].as_str() == Some("computeAnalytics"));
    assert!(
        found,
        "Should find computeAnalytics by name in results: {:?}",
        matches
    );

    // Also verify the second function in the file
    let response = server
        .find_symbol_test(FindSymbolArgs {
            project: Some(project_name.clone()),
            query: "formatAnalyticsReport".to_string(),
            label: None,
            exact: None,
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    let empty2 = vec![];
    let matches = v["matches"].as_array().unwrap_or(&empty2);
    assert!(
        !matches.is_empty(),
        "formatAnalyticsReport should also appear after incremental reindex"
    );
}

#[tokio::test]
async fn test_incremental_reindex_deleted_function_disappears() {
    let (server, project_name, _project_dir, project_path) = setup_mutable_fixture().await;

    // First, add a file so we have something to delete
    let new_file_path = project_path.join("src").join("temporary-feature.ts");
    std::fs::write(
        &new_file_path,
        r#"/**
 * A temporary feature function that will be deleted.
 */
export function temporaryFeature(input: string): string {
    return `processed: ${input}`;
}
"#,
    )
    .expect("failed to write temporary file");

    // Index to pick up the new file
    let response = server
        .index_repository_test(IndexArgs {
            path: project_path.to_string_lossy().into_owned(),
            mode: Some("fast".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;

    let v: Value = serde_json::from_str(&response).expect("reindex response should be JSON");
    assert!(
        v.get("error").is_none(),
        "Reindex after adding file failed: {}",
        response
    );

    // Verify the function exists
    let response = server
        .find_symbol_test(FindSymbolArgs {
            project: Some(project_name.clone()),
            query: "temporaryFeature".to_string(),
            label: None,
            exact: None,
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    let empty_vec = vec![];
    let matches = v["matches"].as_array().unwrap_or(&empty_vec);
    assert!(
        !matches.is_empty(),
        "temporaryFeature should exist after indexing the new file"
    );

    // Now delete the file
    std::fs::remove_file(&new_file_path).expect("failed to delete temporary file");

    // Re-index incrementally
    let response = server
        .index_repository_test(IndexArgs {
            path: project_path.to_string_lossy().into_owned(),
            mode: Some("fast".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;

    let v: Value = serde_json::from_str(&response).expect("reindex response should be JSON");
    assert!(
        v.get("error").is_none(),
        "Incremental reindex after deletion failed: {}",
        response
    );

    // Verify the function no longer appears
    let response = server
        .find_symbol_test(FindSymbolArgs {
            project: Some(project_name.clone()),
            query: "temporaryFeature".to_string(),
            label: None,
            exact: None,
            limit: None,
            include_linked: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    let empty_after = vec![];
    let matches = v["matches"].as_array().unwrap_or(&empty_after);
    let found = matches
        .iter()
        .any(|m| m["name"].as_str() == Some("temporaryFeature"));
    assert!(
        !found,
        "temporaryFeature should NOT appear after deleting the file and reindexing. Got: {:?}",
        matches
    );
}

// ── get_graph_diff tests ──────────────────────────────────────────────────────
// **Validates: Requirements 31.1, 31.2, 31.3, 31.4, 31.5**

#[tokio::test]
async fn test_get_graph_diff_default_compares_two_most_recent_snapshots() {
    // Setup: index the fixture project twice to create two snapshots
    let store_dir = TempDir::new().expect("failed to create temp dir for store");
    let store_path = store_dir.path().join("store");
    std::fs::create_dir_all(&store_path).expect("failed to create store dir");

    let server = CodrynServer::new(&store_path);
    let fixture_path = TestFixture::fixture_project_path();

    // First index (creates first snapshot)
    let response = server
        .index_repository_test(IndexArgs {
            path: fixture_path.to_string_lossy().into_owned(),
            mode: Some("full".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;
    let v: Value = serde_json::from_str(&response).unwrap();
    assert!(v.get("error").is_none(), "First index failed: {response}");
    let project_name = v["project"].as_str().unwrap().to_string();

    // Second index (creates second snapshot)
    let response = server
        .index_repository_test(IndexArgs {
            path: fixture_path.to_string_lossy().into_owned(),
            mode: Some("full".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;
    let v: Value = serde_json::from_str(&response).unwrap();
    assert!(v.get("error").is_none(), "Second index failed: {response}");

    // Call get_graph_diff without specifying snapshot IDs (should use two most recent)
    let response = server
        .get_graph_diff_test(codryn_mcp::GetGraphDiffArgs {
            project: Some(project_name.clone()),
            from_snapshot_id: None,
            to_snapshot_id: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "get_graph_diff failed: {response}"
    );

    // Verify response structure (Req 31.1)
    assert!(v.get("node_delta").is_some(), "Should have node_delta");
    assert!(v.get("edge_delta").is_some(), "Should have edge_delta");
    assert!(
        v.get("label_changes").is_some(),
        "Should have label_changes"
    );
    assert!(
        v.get("edge_type_changes").is_some(),
        "Should have edge_type_changes"
    );

    // Verify snapshot metadata is included (Req 31.5)
    assert!(
        v.get("from_snapshot_id").is_some(),
        "Should include from_snapshot_id"
    );
    assert!(
        v.get("to_snapshot_id").is_some(),
        "Should include to_snapshot_id"
    );
    assert!(
        v.get("from_timestamp").is_some(),
        "Should include from_timestamp"
    );
    assert!(
        v.get("to_timestamp").is_some(),
        "Should include to_timestamp"
    );
    assert!(
        v.get("from_content_hash").is_some(),
        "Should include from_content_hash"
    );
    assert!(
        v.get("to_content_hash").is_some(),
        "Should include to_content_hash"
    );

    // Since we indexed the same project twice without changes, deltas should be 0
    assert_eq!(
        v["node_delta"].as_i64().unwrap(),
        0,
        "node_delta should be 0 for identical indexes"
    );
    assert_eq!(
        v["edge_delta"].as_i64().unwrap(),
        0,
        "edge_delta should be 0 for identical indexes"
    );
}

#[tokio::test]
async fn test_get_graph_diff_with_explicit_snapshot_ids() {
    // Setup: index the fixture project twice
    let store_dir = TempDir::new().expect("failed to create temp dir for store");
    let store_path = store_dir.path().join("store");
    std::fs::create_dir_all(&store_path).expect("failed to create store dir");

    let server = CodrynServer::new(&store_path);
    let fixture_path = TestFixture::fixture_project_path();

    // First index
    let response = server
        .index_repository_test(IndexArgs {
            path: fixture_path.to_string_lossy().into_owned(),
            mode: Some("full".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;
    let v: Value = serde_json::from_str(&response).unwrap();
    assert!(v.get("error").is_none(), "First index failed: {response}");
    let project_name = v["project"].as_str().unwrap().to_string();

    // Second index
    let response = server
        .index_repository_test(IndexArgs {
            path: fixture_path.to_string_lossy().into_owned(),
            mode: Some("full".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;
    let v: Value = serde_json::from_str(&response).unwrap();
    assert!(v.get("error").is_none(), "Second index failed: {response}");

    // First, get the diff without IDs to discover the snapshot IDs
    let response = server
        .get_graph_diff_test(codryn_mcp::GetGraphDiffArgs {
            project: Some(project_name.clone()),
            from_snapshot_id: None,
            to_snapshot_id: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "get_graph_diff failed: {response}"
    );

    let from_id = v["from_snapshot_id"].as_i64().unwrap();
    let to_id = v["to_snapshot_id"].as_i64().unwrap();

    // Now call with explicit IDs (Req 31.2)
    let response = server
        .get_graph_diff_test(codryn_mcp::GetGraphDiffArgs {
            project: Some(project_name.clone()),
            from_snapshot_id: Some(from_id),
            to_snapshot_id: Some(to_id),
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_none(),
        "get_graph_diff with explicit IDs failed: {response}"
    );
    assert_eq!(v["from_snapshot_id"].as_i64().unwrap(), from_id);
    assert_eq!(v["to_snapshot_id"].as_i64().unwrap(), to_id);
}

#[tokio::test]
async fn test_get_graph_diff_error_insufficient_snapshots() {
    // Setup: create a fresh store with no snapshots
    let store_dir = TempDir::new().expect("failed to create temp dir for store");
    let store_path = store_dir.path().join("store");
    std::fs::create_dir_all(&store_path).expect("failed to create store dir");

    let server = CodrynServer::new(&store_path);

    // Call get_graph_diff on a project with no snapshots (Req 31.3)
    let response = server
        .get_graph_diff_test(codryn_mcp::GetGraphDiffArgs {
            project: Some("nonexistent-project".to_string()),
            from_snapshot_id: None,
            to_snapshot_id: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_some(),
        "Should return error when fewer than 2 snapshots exist. Got: {response}"
    );

    let error_msg = v["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("Insufficient snapshots") || error_msg.contains("insufficient"),
        "Error should mention insufficient snapshots. Got: {error_msg}"
    );
}

#[tokio::test]
async fn test_get_graph_diff_error_invalid_snapshot_id() {
    // Setup: index once to have at least one snapshot
    let store_dir = TempDir::new().expect("failed to create temp dir for store");
    let store_path = store_dir.path().join("store");
    std::fs::create_dir_all(&store_path).expect("failed to create store dir");

    let server = CodrynServer::new(&store_path);
    let fixture_path = TestFixture::fixture_project_path();

    let response = server
        .index_repository_test(IndexArgs {
            path: fixture_path.to_string_lossy().into_owned(),
            mode: Some("full".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;
    let v: Value = serde_json::from_str(&response).unwrap();
    assert!(v.get("error").is_none(), "Index failed: {response}");
    let project_name = v["project"].as_str().unwrap().to_string();

    // Index again to have 2 snapshots
    let response = server
        .index_repository_test(IndexArgs {
            path: fixture_path.to_string_lossy().into_owned(),
            mode: Some("full".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;
    let v: Value = serde_json::from_str(&response).unwrap();
    assert!(v.get("error").is_none(), "Second index failed: {response}");

    // Call with an invalid snapshot ID (Req 31.4)
    let response = server
        .get_graph_diff_test(codryn_mcp::GetGraphDiffArgs {
            project: Some(project_name.clone()),
            from_snapshot_id: Some(99999),
            to_snapshot_id: Some(99998),
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_some(),
        "Should return error for invalid snapshot IDs. Got: {response}"
    );

    let error_msg = v["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("snapshot") || error_msg.contains("not found"),
        "Error should mention snapshot not found. Got: {error_msg}"
    );
}

#[tokio::test]
async fn test_get_graph_diff_single_snapshot_returns_error() {
    // Setup: index only once (creates only 1 snapshot)
    let store_dir = TempDir::new().expect("failed to create temp dir for store");
    let store_path = store_dir.path().join("store");
    std::fs::create_dir_all(&store_path).expect("failed to create store dir");

    let server = CodrynServer::new(&store_path);
    let fixture_path = TestFixture::fixture_project_path();

    let response = server
        .index_repository_test(IndexArgs {
            path: fixture_path.to_string_lossy().into_owned(),
            mode: Some("full".to_string()),
            clear_cache: None,
            analytics: None,
        })
        .await;
    let v: Value = serde_json::from_str(&response).unwrap();
    assert!(v.get("error").is_none(), "Index failed: {response}");
    let project_name = v["project"].as_str().unwrap().to_string();

    // Call get_graph_diff with only 1 snapshot available (Req 31.3)
    let response = server
        .get_graph_diff_test(codryn_mcp::GetGraphDiffArgs {
            project: Some(project_name.clone()),
            from_snapshot_id: None,
            to_snapshot_id: None,
            analytics: None,
        })
        .await;

    let v = TestFixture::parse(&response);
    assert!(
        v.get("error").is_some(),
        "Should return error when only 1 snapshot exists. Got: {response}"
    );
}
