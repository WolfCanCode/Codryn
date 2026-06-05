use codryn_cli::query_tool::{list_tools, run_tool};
use proptest::prelude::*;
use std::path::Path;

// ─── Property 12: Unrecognized Tool Error Contains Tool List ─────────────────

/// **Validates: Requirements 4.3**
///
/// Property 12: Unrecognized Tool Error Contains Tool List
/// For any string that is not in the registered tool name list,
/// `run_tool` SHALL return an error that contains both the unrecognized
/// name AND at least one valid tool name from the available list.
/// Strategy that generates random strings guaranteed NOT to be in list_tools().
/// We use alphanumeric strings with a prefix that no tool name starts with.
fn unrecognized_tool_name_strategy() -> impl Strategy<Value = String> {
    // Generate random strings and filter out any that happen to match a registered tool
    "[a-z][a-z0-9_]{2,30}".prop_filter("must not be a registered tool name", |name| {
        let tools = list_tools();
        !tools.contains(&name.as_str())
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn unrecognized_tool_returns_error_with_name_and_valid_tools(
        tool_name in unrecognized_tool_name_strategy()
    ) {
        // Use a non-existent store dir — the tool validation happens before store access
        let fake_store_dir = Path::new("/nonexistent/store/path");

        // Call run_tool with the unrecognized tool name
        let result = run_tool(&tool_name, &[], false, fake_store_dir);

        // Assert it returns an error
        prop_assert!(result.is_err(), "run_tool should fail for unrecognized tool '{}'", tool_name);

        let err_msg = result.unwrap_err().to_string();

        // Assert the error message contains the unrecognized tool name
        prop_assert!(
            err_msg.contains(&tool_name),
            "Error message should contain the unrecognized tool name '{}', got: {}",
            tool_name,
            err_msg
        );

        // Assert the error message contains at least one valid tool name
        let valid_tools = list_tools();
        let contains_valid_tool = valid_tools.iter().any(|valid| err_msg.contains(valid));
        prop_assert!(
            contains_valid_tool,
            "Error message should contain at least one valid tool name from {:?}, got: {}",
            &valid_tools[..5], // show first 5 for brevity
            err_msg
        );
    }
}
