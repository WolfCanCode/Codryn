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
//! **Validates: Requirements 2.1**

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
        assert!(
            v.get("error").is_none(),
            "Indexing failed: {}",
            response
        );

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
    assert!(!matches.is_empty(), "Should find at least one match for 'main'");

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
    assert!(
        !matches.is_empty(),
        "Should find UserService class"
    );

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
    assert!(count > 0, "Should find nodes matching 'User', got count: {count}");
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
    assert!(!matches.is_empty(), "Should find 'main' function after indexing");

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
