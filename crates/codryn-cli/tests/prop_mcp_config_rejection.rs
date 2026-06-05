//! Property 13: MCP Config Rejection Preserves Content
//!
//! **Validates: Requirements 5.2**
//!
//! For any existing mcp.json file content and for any set of proposed changes,
//! if the user rejects the confirmation prompt, the file content SHALL be
//! byte-for-byte identical after the operation completes.

use codryn_cli::mcp_config::McpConfigManager;
use codryn_cli::prompter::{MockPrompter, MockResponse};
use proptest::prelude::*;
use std::path::Path;
use tempfile::TempDir;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Generate a random valid JSON config with an mcpServers object.
/// The mcpServers object may or may not already contain a codryn entry.
fn valid_mcp_config_strategy() -> impl Strategy<Value = String> {
    // Generate random additional server names and command values
    let server_name = "[a-z][a-z0-9-]{1,15}";
    let command_value = "/[a-z]{2,8}/[a-z]{2,8}";

    // Generate 0-3 additional server entries
    prop::collection::vec((server_name, command_value), 0..4).prop_map(|servers| {
        let mut mcp_servers = serde_json::Map::new();
        for (name, cmd) in &servers {
            mcp_servers.insert(
                name.clone(),
                serde_json::json!({
                    "command": cmd
                }),
            );
        }

        let config = serde_json::json!({
            "mcpServers": serde_json::Value::Object(mcp_servers)
        });

        serde_json::to_string_pretty(&config).unwrap()
    })
}

/// Generate a random binary path to propose as the MCP server binary.
fn binary_path_strategy() -> impl Strategy<Value = String> {
    "/[a-z]{3,8}/[a-z]{3,8}/[a-z]{2,6}".prop_map(|s| s)
}

// ─── Property 13: MCP Config Rejection Preserves Content ─────────────────────

/// **Validates: Requirements 5.2**
mod property13_mcp_config_rejection_preserves_content {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn rejection_preserves_file_content(
            config_content in valid_mcp_config_strategy(),
            binary_path in binary_path_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");
            let config_path = tmp.path().join("mcp.json");

            // Write the initial config content
            std::fs::write(&config_path, &config_content).unwrap();

            // Record the original bytes
            let original_bytes = std::fs::read(&config_path).unwrap();

            // Create a MockPrompter that rejects the confirmation
            let prompter = MockPrompter::new(vec![MockResponse::Confirm(false)]);
            let manager = McpConfigManager::new(&prompter);

            // Attempt to add — this should propose changes and the user rejects
            let result = manager.add(
                Path::new(&binary_path),
                &[config_path.clone()],
            );

            // The operation should succeed (rejection is not an error)
            prop_assert!(result.is_ok(), "add() should not error on rejection: {:?}", result.err());

            // Assert the file is byte-for-byte identical
            let after_bytes = std::fs::read(&config_path).unwrap();
            prop_assert_eq!(
                original_bytes,
                after_bytes,
                "File content must be byte-for-byte identical after rejection"
            );
        }
    }
}
