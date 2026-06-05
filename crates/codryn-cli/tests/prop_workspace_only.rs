//! Property 18: Workspace-Only Scope Limitation
//!
//! **Validates: Requirements 6.4**
//!
//! For any uninstall execution with `--workspace-only`, only files located within
//! the current workspace directory SHALL be modified or removed; all files outside
//! the workspace (global steering, MCP configs, preferences file, data directories)
//! SHALL remain unchanged.

use codryn_cli::uninstall::{
    execute_uninstall, ArtifactCategory, InstalledArtifact, RemovalResult,
};
use proptest::prelude::*;
use std::fs;
use tempfile::TempDir;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Generate a valid filename component (alphanumeric + hyphens/underscores + .md extension).
fn filename_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{2,12}\\.md".prop_map(|s| s)
}

/// Generate a count of workspace artifacts (1..=5).
fn workspace_artifact_count_strategy() -> impl Strategy<Value = usize> {
    1..=5usize
}

/// Generate a count of global (outside workspace) artifacts (1..=5).
fn global_artifact_count_strategy() -> impl Strategy<Value = usize> {
    1..=5usize
}

// ─── Property 18: Workspace-Only Scope Limitation ────────────────────────────

/// **Validates: Requirements 6.4**
mod property18_workspace_only_scope {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]

        /// Artifacts INSIDE the workspace are removed (steering/skill files only);
        /// artifacts OUTSIDE the workspace remain unchanged.
        #[test]
        fn workspace_only_removes_only_workspace_artifacts(
            ws_count in workspace_artifact_count_strategy(),
            global_count in global_artifact_count_strategy(),
            ws_filenames in prop::collection::vec(filename_strategy(), 1..=5),
            global_filenames in prop::collection::vec(filename_strategy(), 1..=5),
        ) {
            let workspace_dir = TempDir::new().expect("failed to create workspace temp dir");
            let global_dir = TempDir::new().expect("failed to create global temp dir");

            let ws_count = ws_count.min(ws_filenames.len());
            let global_count = global_count.min(global_filenames.len());

            // Create workspace steering files
            let ws_steering_dir = workspace_dir.path().join(".kiro").join("steering");
            fs::create_dir_all(&ws_steering_dir).expect("failed to create workspace steering dir");

            let mut artifacts = Vec::new();

            // Place workspace artifacts (steering and skill files inside workspace)
            for i in 0..ws_count {
                let filename = &ws_filenames[i];
                let file_path = ws_steering_dir.join(filename);
                fs::write(&file_path, format!("workspace content {}", i))
                    .expect("failed to write workspace file");
                artifacts.push(InstalledArtifact {
                    category: ArtifactCategory::SteeringFile,
                    path: file_path,
                    description: format!("Workspace steering file {}", i),
                });
            }

            // Place global artifacts (outside workspace)
            let global_steering_dir = global_dir.path().join("steering");
            fs::create_dir_all(&global_steering_dir)
                .expect("failed to create global steering dir");
            let global_mcp_dir = global_dir.path().join("mcp-configs");
            fs::create_dir_all(&global_mcp_dir)
                .expect("failed to create global MCP config dir");
            let global_data_dir = global_dir.path().join("data");
            fs::create_dir_all(&global_data_dir)
                .expect("failed to create global data dir");

            // Create global steering files
            for i in 0..global_count {
                let filename = &global_filenames[i];
                let file_path = global_steering_dir.join(filename);
                fs::write(&file_path, format!("global steering content {}", i))
                    .expect("failed to write global steering file");
                artifacts.push(InstalledArtifact {
                    category: ArtifactCategory::SteeringFile,
                    path: file_path,
                    description: format!("Global steering file {}", i),
                });
            }

            // Add a global MCP config entry (outside workspace)
            let mcp_config_path = global_mcp_dir.join("mcp.json");
            fs::write(
                &mcp_config_path,
                r#"{"mcpServers":{"codryn":{"command":"codryn"}}}"#,
            )
            .expect("failed to write MCP config");
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::McpConfigEntry,
                path: mcp_config_path.clone(),
                description: "Global MCP config entry".to_string(),
            });

            // Add a global data directory (outside workspace)
            let data_subdir = global_data_dir.join("graph.db");
            fs::write(&data_subdir, "fake database content")
                .expect("failed to write data file");
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::DataDirectory,
                path: global_data_dir.clone(),
                description: "Global data directory".to_string(),
            });

            // Record the content of global files before uninstall
            let global_steering_contents_before: Vec<String> = (0..global_count)
                .map(|i| {
                    fs::read_to_string(global_steering_dir.join(&global_filenames[i]))
                        .expect("failed to read global file before uninstall")
                })
                .collect();
            let mcp_content_before =
                fs::read_to_string(&mcp_config_path).expect("failed to read MCP config");
            let data_content_before =
                fs::read_to_string(&data_subdir).expect("failed to read data file");

            // Execute uninstall with workspace_only=true
            let results = execute_uninstall(
                &artifacts,
                false,               // keep_data = false
                true,                // workspace_only = true
                Some(workspace_dir.path()),
            );

            // Assertions:
            // 1. Workspace steering files should be removed (Success)
            for i in 0..ws_count {
                let result = &results[i];
                prop_assert!(
                    result.is_success(),
                    "workspace artifact {} should have been removed: {:?}",
                    i,
                    result
                );
                prop_assert!(
                    !artifacts[i].path.exists(),
                    "workspace file {} should no longer exist on disk",
                    i
                );
            }

            // 2. Global steering files should be skipped (unchanged)
            for i in 0..global_count {
                let result_idx = ws_count + i;
                let result = &results[result_idx];
                prop_assert!(
                    matches!(result, RemovalResult::Skipped { .. }),
                    "global steering file {} should have been skipped: {:?}",
                    i,
                    result
                );
                // Verify file content is unchanged
                let content_after =
                    fs::read_to_string(global_steering_dir.join(&global_filenames[i]))
                        .expect("global file should still be readable");
                prop_assert_eq!(
                    &content_after,
                    &global_steering_contents_before[i],
                    "global steering file {} content should be unchanged",
                    i
                );
            }

            // 3. MCP config entry should be skipped (unchanged)
            let mcp_result_idx = ws_count + global_count;
            let mcp_result = &results[mcp_result_idx];
            prop_assert!(
                matches!(mcp_result, RemovalResult::Skipped { .. }),
                "MCP config entry should have been skipped: {:?}",
                mcp_result
            );
            let mcp_content_after =
                fs::read_to_string(&mcp_config_path).expect("MCP config should still exist");
            prop_assert_eq!(
                &mcp_content_after,
                &mcp_content_before,
                "MCP config content should be unchanged"
            );

            // 4. Data directory should be skipped (unchanged)
            let data_result_idx = mcp_result_idx + 1;
            let data_result = &results[data_result_idx];
            prop_assert!(
                matches!(data_result, RemovalResult::Skipped { .. }),
                "Data directory should have been skipped: {:?}",
                data_result
            );
            let data_content_after =
                fs::read_to_string(&data_subdir).expect("data file should still exist");
            prop_assert_eq!(
                &data_content_after,
                &data_content_before,
                "data directory content should be unchanged"
            );
        }

        /// Even MCP config entries and data directories INSIDE the workspace
        /// are skipped in workspace-only mode (only steering/skill files removed).
        #[test]
        fn workspace_only_skips_mcp_and_data_even_inside_workspace(
            filename in filename_strategy(),
        ) {
            let workspace_dir = TempDir::new().expect("failed to create temp dir");

            // Create a steering file inside workspace
            let steering_dir = workspace_dir.path().join(".kiro").join("steering");
            fs::create_dir_all(&steering_dir).expect("failed to create steering dir");
            let steering_file = steering_dir.join(&filename);
            fs::write(&steering_file, "steering content").expect("failed to write steering file");

            // Create an MCP config inside workspace
            let mcp_config = workspace_dir.path().join("mcp.json");
            fs::write(
                &mcp_config,
                r#"{"mcpServers":{"codryn":{"command":"codryn"}}}"#,
            )
            .expect("failed to write MCP config");

            // Create a data directory inside workspace
            let data_dir = workspace_dir.path().join(".codryn-data");
            fs::create_dir_all(&data_dir).expect("failed to create data dir");
            let db_file = data_dir.join("graph.db");
            fs::write(&db_file, "database content").expect("failed to write db file");

            let artifacts = vec![
                InstalledArtifact {
                    category: ArtifactCategory::SteeringFile,
                    path: steering_file.clone(),
                    description: "Workspace steering".to_string(),
                },
                InstalledArtifact {
                    category: ArtifactCategory::McpConfigEntry,
                    path: mcp_config.clone(),
                    description: "Workspace MCP config".to_string(),
                },
                InstalledArtifact {
                    category: ArtifactCategory::DataDirectory,
                    path: data_dir.clone(),
                    description: "Workspace data directory".to_string(),
                },
            ];

            let results = execute_uninstall(
                &artifacts,
                false,
                true,
                Some(workspace_dir.path()),
            );

            prop_assert_eq!(results.len(), 3);

            // Steering file inside workspace: should be removed
            prop_assert!(
                results[0].is_success(),
                "steering file inside workspace should be removed"
            );
            prop_assert!(!steering_file.exists(), "steering file should be gone");

            // MCP config inside workspace: should be SKIPPED (workspace-only only removes steering/skill)
            prop_assert!(
                matches!(results[1], RemovalResult::Skipped { .. }),
                "MCP config should be skipped in workspace-only mode: {:?}",
                results[1]
            );
            prop_assert!(mcp_config.exists(), "MCP config should still exist");

            // Data directory inside workspace: should be SKIPPED
            prop_assert!(
                matches!(results[2], RemovalResult::Skipped { .. }),
                "Data directory should be skipped in workspace-only mode: {:?}",
                results[2]
            );
            prop_assert!(data_dir.exists(), "data directory should still exist");
        }
    }
}
