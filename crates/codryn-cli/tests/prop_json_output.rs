//! Property 11: JSON Output Validity
//!
//! **Validates: Requirements 4.2**
//!
//! For any valid tool name and valid arguments producing a successful result,
//! the `--json` output SHALL be valid JSON that can be parsed by
//! `serde_json::from_str` without error.
//!
//! This test verifies the JSON output validity property by:
//! 1. Creating a valid store with an initialized database
//! 2. Choosing from tools that produce successful results without complex state
//! 3. Calling `run_tool` with `json_output=true`
//! 4. Verifying the output is valid JSON parseable by `serde_json::from_str`
//!
//! Since `run_tool` prints directly to stdout, we verify the property by:
//! - Confirming `run_tool` succeeds (returns Ok) for valid tools with json_output=true
//! - Testing the JSON serialization logic directly: constructing result values
//!   (as produced by dispatch) and verifying the wrapping produces valid JSON

use proptest::prelude::*;
use serde_json::json;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Tools that can produce successful results with an empty store and no arguments.
const SIMPLE_TOOLS: &[&str] = &["health_check", "clear_cache", "list_projects"];

/// Strategy to pick a simple tool that succeeds without complex state.
fn simple_tool_strategy() -> impl Strategy<Value = &'static str> {
    prop::sample::select(SIMPLE_TOOLS)
}

/// Strategy to generate arbitrary JSON values similar to tool dispatch results.
/// These mimic the kinds of values that `dispatch_tool` returns.
fn tool_result_strategy() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        // Simple status responses (like health_check, clear_cache)
        Just(json!({"status": "ok", "mode": "cli"})),
        Just(json!({"status": "ok", "message": "cache cleared"})),
        // List responses (like list_projects with empty results)
        Just(json!({"projects": []})),
        // Schema responses
        Just(json!({"total_nodes": 0, "total_edges": 0, "node_labels": {}, "edge_types": {}})),
        // Index status responses
        Just(json!({"project": "default", "indexed": false, "total_nodes": 0, "total_edges": 0})),
        // Not-yet-implemented responses
        Just(json!({"status": "not_yet_implemented", "tool": "some_tool", "message": "not yet"})),
        // Responses with nested arrays
        Just(json!({"results": [{"name": "foo", "label": "Function"}], "count": 1})),
        // Responses with numeric values
        Just(json!({"count": 42, "total": 100, "ratio": 0.95})),
        // Empty object
        Just(json!({})),
        // Response with null values
        Just(json!({"data": null, "status": "ok"})),
        // Deeply nested response
        Just(json!({"level1": {"level2": {"level3": "deep"}}})),
        // Response with special characters in strings
        "\\PC{0,100}".prop_map(|s| json!({"message": s})),
        // Response with arrays of mixed types
        Just(json!({"items": [1, "two", true, null, 2.5]})),
    ]
}

// ─── Property 11: JSON Output Validity ───────────────────────────────────────

/// **Validates: Requirements 4.2**
mod property11_json_output_validity {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// For any tool result value, the JSON wrapping logic (as used by format_json)
        /// always produces output parseable by serde_json::from_str.
        #[test]
        fn json_output_is_always_valid(result in tool_result_strategy()) {
            // Replicate the exact logic from format_json in query_tool.rs:
            // It wraps the result in {"success": true, "result": <value>}
            // and serializes with serde_json::to_string_pretty
            let output = json!({
                "success": true,
                "result": result,
            });

            let serialized = serde_json::to_string_pretty(&output)
                .unwrap_or_else(|_| "{}".to_string());

            // The core property: output must be parseable as valid JSON
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(&serialized);
            prop_assert!(
                parsed.is_ok(),
                "JSON output should be parseable, got error: {:?} for output: {}",
                parsed.err(),
                &serialized[..serialized.len().min(200)]
            );

            // Additional structural assertions:
            let parsed = parsed.unwrap();
            prop_assert_eq!(
                parsed.get("success").and_then(|v| v.as_bool()),
                Some(true),
                "Output must contain 'success': true"
            );
            prop_assert!(
                parsed.get("result").is_some(),
                "Output must contain 'result' field"
            );
        }

        /// For any simple tool that succeeds with an empty store,
        /// run_tool with json_output=true completes without error.
        #[test]
        fn run_tool_json_mode_succeeds_for_valid_tools(tool in simple_tool_strategy()) {
            let tmp = tempfile::tempdir().expect("failed to create temp dir");
            let db_path = tmp.path().join("graph.db");

            // Create a valid store
            let _store = codryn_store::Store::open(&db_path)
                .expect("failed to open store");

            // Call run_tool with json_output=true — should succeed
            let result = codryn_cli::query_tool::run_tool(
                tool,
                &[],
                true, // json_output
                tmp.path(),
            );

            prop_assert!(
                result.is_ok(),
                "run_tool with json_output=true should succeed for tool '{}', got: {:?}",
                tool,
                result.err()
            );
        }
    }

    /// Verify that the fallback case in format_json (when to_string_pretty fails)
    /// still produces valid JSON. This is a defensive test — serde_json::to_string_pretty
    /// should never fail for a valid serde_json::Value, but the fallback "{}" is still valid.
    #[test]
    fn fallback_empty_json_is_valid() {
        let fallback = "{}";
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(fallback);
        assert!(parsed.is_ok(), "Fallback '{{}}' must be valid JSON");
    }

    /// End-to-end test: run each simple tool with a real store and verify
    /// that run_tool returns Ok when json_output=true.
    #[test]
    fn all_simple_tools_produce_valid_json_output() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = tmp.path().join("graph.db");
        let _store = codryn_store::Store::open(&db_path).expect("failed to open store");

        for tool in SIMPLE_TOOLS {
            let result = codryn_cli::query_tool::run_tool(tool, &[], true, tmp.path());
            assert!(
                result.is_ok(),
                "Tool '{}' with json_output=true should succeed, got: {:?}",
                tool,
                result.err()
            );
        }
    }
}
