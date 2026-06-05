//! Property 17: Keep-Data Preserves Database
//!
//! **Validates: Requirements 6.3**
//!
//! For any uninstall execution with `--keep-data`, the graph database file and
//! index data directory SHALL remain present on disk after completion, while all
//! other artifacts (steering, skills, MCP config entries) are removed.

use codryn_cli::uninstall::{
    execute_uninstall, ArtifactCategory, InstalledArtifact, RemovalResult,
};
use proptest::prelude::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Generate a random file name for steering/skill files.
fn artifact_filename_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{2,15}\\.md"
}

/// Generate a random directory name for data directories.
fn data_dirname_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{2,10}"
}

// ─── Property 17: Keep-Data Preserves Database ───────────────────────────────

/// **Validates: Requirements 6.3**
mod property17_keep_data_preserves_database {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]

        #[test]
        fn keep_data_preserves_data_directory_and_removes_others(
            steering_names in prop::collection::vec(artifact_filename_strategy(), 1..4),
            skill_names in prop::collection::vec(artifact_filename_strategy(), 1..4),
            mcp_count in 1..3usize,
            data_dir_name in data_dirname_strategy(),
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");
            let root = tmp.path();

            // Create steering file artifacts on disk
            let steering_dir = root.join("steering");
            fs::create_dir_all(&steering_dir).unwrap();
            let steering_artifacts: Vec<InstalledArtifact> = steering_names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let path = steering_dir.join(format!("{}_{}", i, name));
                    fs::write(&path, format!("steering content {}", i)).unwrap();
                    InstalledArtifact {
                        category: ArtifactCategory::SteeringFile,
                        path,
                        description: format!("Steering file {}", i),
                    }
                })
                .collect();

            // Create skill file artifacts on disk
            let skills_dir = root.join("skills");
            fs::create_dir_all(&skills_dir).unwrap();
            let skill_artifacts: Vec<InstalledArtifact> = skill_names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let path = skills_dir.join(format!("{}_{}", i, name));
                    fs::write(&path, format!("skill content {}", i)).unwrap();
                    InstalledArtifact {
                        category: ArtifactCategory::SkillFile,
                        path,
                        description: format!("Skill file {}", i),
                    }
                })
                .collect();

            // Create MCP config entry artifacts on disk
            let mcp_dir = root.join("mcp_configs");
            fs::create_dir_all(&mcp_dir).unwrap();
            let mcp_artifacts: Vec<InstalledArtifact> = (0..mcp_count)
                .map(|i| {
                    let path = mcp_dir.join(format!("mcp_{}.json", i));
                    // Write a valid MCP config with our entry
                    let config = serde_json::json!({
                        "mcpServers": {
                            "codryn": {
                                "command": "/usr/local/bin/codryn"
                            },
                            "other-server": {
                                "command": "other"
                            }
                        }
                    });
                    fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
                    InstalledArtifact {
                        category: ArtifactCategory::McpConfigEntry,
                        path,
                        description: format!("MCP config entry {}", i),
                    }
                })
                .collect();

            // Create the DataDirectory artifact on disk
            let data_path = root.join(&data_dir_name);
            fs::create_dir_all(data_path.join("subdir")).unwrap();
            fs::write(data_path.join("graph.db"), "graph database content").unwrap();
            fs::write(data_path.join("subdir").join("index.bin"), "index data").unwrap();

            let data_artifact = InstalledArtifact {
                category: ArtifactCategory::DataDirectory,
                path: data_path.clone(),
                description: "CBM data directory".to_string(),
            };

            // Combine all artifacts
            let mut all_artifacts: Vec<InstalledArtifact> = Vec::new();
            all_artifacts.extend(steering_artifacts.clone());
            all_artifacts.extend(skill_artifacts.clone());
            all_artifacts.extend(mcp_artifacts.clone());
            all_artifacts.push(data_artifact);

            // Execute uninstall with keep_data=true
            let results = execute_uninstall(&all_artifacts, true, false, None);

            // Assert: results count matches artifact count
            prop_assert_eq!(
                results.len(),
                all_artifacts.len(),
                "should have one result per artifact"
            );

            // Assert: DataDirectory artifacts still exist on disk (were skipped)
            prop_assert!(
                data_path.exists(),
                "Data directory should still exist after keep_data uninstall"
            );
            prop_assert!(
                data_path.join("graph.db").exists(),
                "graph.db should still exist after keep_data uninstall"
            );
            prop_assert!(
                data_path.join("subdir").join("index.bin").exists(),
                "index data should still exist after keep_data uninstall"
            );

            // Assert: SteeringFile artifacts were removed
            for artifact in &steering_artifacts {
                prop_assert!(
                    !artifact.path.exists(),
                    "Steering file should be removed: {:?}",
                    artifact.path
                );
            }

            // Assert: SkillFile artifacts were removed
            for artifact in &skill_artifacts {
                prop_assert!(
                    !artifact.path.exists(),
                    "Skill file should be removed: {:?}",
                    artifact.path
                );
            }

            // Assert: McpConfigEntry artifacts were processed (entry removed from JSON)
            for artifact in &mcp_artifacts {
                // The file still exists (only the entry is removed from it)
                prop_assert!(
                    artifact.path.exists(),
                    "MCP config file should still exist (entry removed, not file)"
                );
                let content = fs::read_to_string(&artifact.path).unwrap();
                prop_assert!(
                    !content.contains("codryn"),
                    "codryn entry should be removed from MCP config: {:?}",
                    artifact.path
                );
                // Other servers should remain
                prop_assert!(
                    content.contains("other-server"),
                    "other-server entry should be preserved in MCP config: {:?}",
                    artifact.path
                );
            }

            // Assert: RemovalResult for DataDirectory entries should be Skipped
            let data_results: Vec<&RemovalResult> = results
                .iter()
                .filter(|r| r.path() == data_path)
                .collect();
            prop_assert!(
                !data_results.is_empty(),
                "Should have a result for the data directory"
            );
            for result in &data_results {
                prop_assert!(
                    matches!(result, RemovalResult::Skipped { .. }),
                    "DataDirectory result should be Skipped, got: {:?}",
                    result
                );
            }

            // Assert: RemovalResult for non-data artifacts should be Success
            let non_data_results: Vec<&RemovalResult> = results
                .iter()
                .filter(|r| r.path() != data_path)
                .collect();
            for result in &non_data_results {
                prop_assert!(
                    result.is_success(),
                    "Non-data artifact removal should succeed, got: {:?}",
                    result
                );
            }
        }

        #[test]
        fn keep_data_skips_multiple_data_directories(
            data_dir_names in prop::collection::vec(data_dirname_strategy(), 1..3),
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");
            let root = tmp.path();

            // Create multiple data directory artifacts
            let mut artifacts: Vec<InstalledArtifact> = Vec::new();
            let mut data_paths: Vec<PathBuf> = Vec::new();

            for (i, name) in data_dir_names.iter().enumerate() {
                let data_path = root.join(format!("{}_{}", i, name));
                fs::create_dir_all(&data_path).unwrap();
                fs::write(data_path.join("data.db"), format!("db content {}", i)).unwrap();
                data_paths.push(data_path.clone());
                artifacts.push(InstalledArtifact {
                    category: ArtifactCategory::DataDirectory,
                    path: data_path,
                    description: format!("Data directory {}", i),
                });
            }

            // Also add a steering file to be removed
            let steering_path = root.join("test_steering.md");
            fs::write(&steering_path, "steering content").unwrap();
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::SteeringFile,
                path: steering_path.clone(),
                description: "Test steering file".to_string(),
            });

            // Execute with keep_data=true
            let results = execute_uninstall(&artifacts, true, false, None);

            // All data directories should still exist
            for data_path in &data_paths {
                prop_assert!(
                    data_path.exists(),
                    "Data directory should be preserved: {:?}",
                    data_path
                );
            }

            // Steering file should be removed
            prop_assert!(
                !steering_path.exists(),
                "Steering file should be removed even with keep_data"
            );

            // Data directory results should all be Skipped
            let data_results: Vec<&RemovalResult> = results
                .iter()
                .filter(|r| data_paths.contains(&r.path().to_path_buf()))
                .collect();
            for result in &data_results {
                prop_assert!(
                    matches!(result, RemovalResult::Skipped { .. }),
                    "All data directory results should be Skipped: {:?}",
                    result
                );
            }
        }
    }
}
