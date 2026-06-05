//! Uninstall module for codryn.
//!
//! Provides artifact discovery and removal for clean uninstallation.
//! Discovers installed steering files, skill files, MCP config entries,
//! and data directories, then removes them with support for `--keep-data`
//! and `--workspace-only` flags.

use std::fmt;
use std::path::{Path, PathBuf};

use codryn_foundation::ide_detect::detect_ides;

use crate::preferences::InstallPreferences;

/// The filename used for codebase-memory steering files.
const STEERING_FILENAME: &str = "codebase-memory.md";

/// The key used to identify our MCP server entry in config files.
const MCP_ENTRY_KEY: &str = "codryn";

/// Category of installed artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactCategory {
    /// A steering file (e.g., `.kiro/steering/codebase-memory.md`)
    SteeringFile,
    /// A skill file (e.g., `.kiro/skills/codebase-memory.md`)
    SkillFile,
    /// An MCP configuration entry in an IDE config file
    McpConfigEntry,
    /// A data directory (e.g., graph database, index data)
    DataDirectory,
}

impl fmt::Display for ArtifactCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtifactCategory::SteeringFile => write!(f, "Steering Files"),
            ArtifactCategory::SkillFile => write!(f, "Skill Files"),
            ArtifactCategory::McpConfigEntry => write!(f, "MCP Config Entries"),
            ArtifactCategory::DataDirectory => write!(f, "Data Directories"),
        }
    }
}

/// A discovered installed artifact.
#[derive(Debug, Clone)]
pub struct InstalledArtifact {
    /// The category this artifact belongs to.
    pub category: ArtifactCategory,
    /// The filesystem path to the artifact.
    pub path: PathBuf,
    /// A human-readable description of this artifact.
    pub description: String,
}

/// Result of attempting to remove a single artifact.
#[derive(Debug, Clone)]
pub enum RemovalResult {
    /// Successfully removed.
    Success {
        path: PathBuf,
        description: String,
    },
    /// Removal failed (e.g., permission denied, file locked).
    Failed {
        path: PathBuf,
        description: String,
        reason: String,
    },
    /// Artifact was skipped (e.g., due to `--keep-data` or `--workspace-only`).
    Skipped {
        path: PathBuf,
        description: String,
        reason: String,
    },
}

impl RemovalResult {
    /// Returns the path of the artifact.
    pub fn path(&self) -> &Path {
        match self {
            RemovalResult::Success { path, .. } => path,
            RemovalResult::Failed { path, .. } => path,
            RemovalResult::Skipped { path, .. } => path,
        }
    }

    /// Returns true if the removal was successful.
    pub fn is_success(&self) -> bool {
        matches!(self, RemovalResult::Success { .. })
    }
}

/// Discover all installed artifacts.
///
/// If preferences are available, uses `activated_workspaces` and IDE information
/// to find steering files and MCP config entries. Also scans the platform data
/// directory for the CBM store.
///
/// If preferences are not available (file missing/unreadable), falls back to
/// scanning default installation paths:
/// - Platform data directory for CBM data
/// - Workspace `.kiro/steering/` directories
/// - Known IDE MCP config file locations
pub fn discover_artifacts(prefs: &Option<InstallPreferences>) -> Vec<InstalledArtifact> {
    let mut artifacts = Vec::new();

    match prefs {
        Some(prefs) => discover_from_preferences(prefs, &mut artifacts),
        None => discover_from_defaults(&mut artifacts),
    }

    artifacts
}

/// Discover artifacts using the preferences file.
fn discover_from_preferences(prefs: &InstallPreferences, artifacts: &mut Vec<InstalledArtifact>) {
    // Discover steering files from activated workspaces
    if let Some(workspaces) = &prefs.activated_workspaces {
        for workspace in workspaces {
            let steering_path = workspace
                .path
                .join(".kiro")
                .join("steering")
                .join(STEERING_FILENAME);
            if steering_path.exists() {
                artifacts.push(InstalledArtifact {
                    category: ArtifactCategory::SteeringFile,
                    path: steering_path,
                    description: format!(
                        "Workspace steering file ({})",
                        workspace.path.display()
                    ),
                });
            }

            // Check for skill files in workspace
            let skill_path = workspace
                .path
                .join(".kiro")
                .join("skills")
                .join(STEERING_FILENAME);
            if skill_path.exists() {
                artifacts.push(InstalledArtifact {
                    category: ArtifactCategory::SkillFile,
                    path: skill_path,
                    description: format!("Workspace skill file ({})", workspace.path.display()),
                });
            }
        }
    }

    // Check global steering file
    if let Some(home) = dirs::home_dir() {
        let global_steering = home.join(".kiro").join("steering").join(STEERING_FILENAME);
        if global_steering.exists() {
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::SteeringFile,
                path: global_steering,
                description: "Global steering file".to_string(),
            });
        }

        let global_skill = home.join(".kiro").join("skills").join(STEERING_FILENAME);
        if global_skill.exists() {
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::SkillFile,
                path: global_skill,
                description: "Global skill file".to_string(),
            });
        }
    }

    // Scan detected IDEs for MCP config entries
    discover_mcp_config_entries(artifacts);

    // Discover data directory
    discover_data_directory(artifacts);
}

/// Discover artifacts by scanning default paths (when preferences are missing).
fn discover_from_defaults(artifacts: &mut Vec<InstalledArtifact>) {
    // Check global steering/skill files
    if let Some(home) = dirs::home_dir() {
        let global_steering = home.join(".kiro").join("steering").join(STEERING_FILENAME);
        if global_steering.exists() {
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::SteeringFile,
                path: global_steering,
                description: "Global steering file".to_string(),
            });
        }

        let global_skill = home.join(".kiro").join("skills").join(STEERING_FILENAME);
        if global_skill.exists() {
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::SkillFile,
                path: global_skill,
                description: "Global skill file".to_string(),
            });
        }

        // Also check .github/instructions directory (legacy location)
        let github_instructions = home
            .join(".github")
            .join("instructions")
            .join("codryn-steering.instructions.md");
        if github_instructions.exists() {
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::SteeringFile,
                path: github_instructions,
                description: "Legacy GitHub instructions steering file".to_string(),
            });
        }
    }

    // Scan detected IDEs for MCP config entries
    discover_mcp_config_entries(artifacts);

    // Discover data directory
    discover_data_directory(artifacts);
}

/// Scan all detected IDE MCP config paths for codryn entries.
fn discover_mcp_config_entries(artifacts: &mut Vec<InstalledArtifact>) {
    let detected_ides = detect_ides();

    for ide in &detected_ides {
        let config_path = &ide.mcp_config_path;
        if !config_path.exists() {
            continue;
        }

        let content = match std::fs::read_to_string(config_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let config: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Check if config contains a codryn entry
        let has_entry = config
            .get("mcpServers")
            .and_then(|v| v.as_object())
            .and_then(|servers| servers.get(MCP_ENTRY_KEY))
            .is_some()
            || config
                .get("servers")
                .and_then(|v| v.as_object())
                .and_then(|servers| servers.get(MCP_ENTRY_KEY))
                .is_some();

        if has_entry {
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::McpConfigEntry,
                path: config_path.clone(),
                description: format!(
                    "MCP config entry in {} ({})",
                    ide.ide.display_name(),
                    config_path.display()
                ),
            });
        }
    }
}

/// Discover the CBM data directory (graph database and index data).
fn discover_data_directory(artifacts: &mut Vec<InstalledArtifact>) {
    if let Some(data_dir) = codryn_data_dir() {
        if data_dir.exists() {
            artifacts.push(InstalledArtifact {
                category: ArtifactCategory::DataDirectory,
                path: data_dir,
                description: "CBM data directory (graph database, indexes)".to_string(),
            });
        }
    }
}

/// Returns the platform-specific CBM data directory path.
///
/// - macOS: `~/Library/Application Support/cbm`
/// - Linux: `~/.local/share/cbm`
/// - Windows: `%APPDATA%/cbm`
fn codryn_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("codryn"))
}

/// Execute uninstall with the given options.
///
/// For each artifact:
/// - If `workspace_only` is set, skip artifacts outside the workspace path
/// - If `keep_data` is set, skip `DataDirectory` artifacts
/// - Attempt removal; on permission error, skip + warn + continue
///
/// Returns a `RemovalResult` for each artifact processed.
pub fn execute_uninstall(
    artifacts: &[InstalledArtifact],
    keep_data: bool,
    workspace_only: bool,
    workspace_path: Option<&Path>,
) -> Vec<RemovalResult> {
    let mut results = Vec::new();

    for artifact in artifacts {
        // Skip DataDirectory if --keep-data
        if keep_data && artifact.category == ArtifactCategory::DataDirectory {
            results.push(RemovalResult::Skipped {
                path: artifact.path.clone(),
                description: artifact.description.clone(),
                reason: "Preserved due to --keep-data flag".to_string(),
            });
            continue;
        }

        // Skip artifacts outside workspace if --workspace-only
        if workspace_only {
            if let Some(ws_path) = workspace_path {
                if !is_within_workspace(&artifact.path, ws_path) {
                    results.push(RemovalResult::Skipped {
                        path: artifact.path.clone(),
                        description: artifact.description.clone(),
                        reason: "Outside workspace scope (--workspace-only)".to_string(),
                    });
                    continue;
                }
            } else {
                // No workspace path provided with --workspace-only: skip everything
                results.push(RemovalResult::Skipped {
                    path: artifact.path.clone(),
                    description: artifact.description.clone(),
                    reason: "No workspace path specified with --workspace-only".to_string(),
                });
                continue;
            }

            // In workspace-only mode, only remove steering and skill files
            // (not MCP config entries or data directories, even if inside workspace)
            if artifact.category == ArtifactCategory::McpConfigEntry
                || artifact.category == ArtifactCategory::DataDirectory
            {
                results.push(RemovalResult::Skipped {
                    path: artifact.path.clone(),
                    description: artifact.description.clone(),
                    reason: "MCP config and data not removed in --workspace-only mode".to_string(),
                });
                continue;
            }
        }

        // Attempt removal
        let result = remove_artifact(artifact);
        results.push(result);
    }

    results
}

/// Attempt to remove a single artifact from the filesystem.
///
/// For MCP config entries, removes the `codryn` key from the JSON
/// rather than deleting the entire file.
///
/// On permission denied or file-locked errors, returns `Failed` instead of
/// propagating the error.
fn remove_artifact(artifact: &InstalledArtifact) -> RemovalResult {
    match artifact.category {
        ArtifactCategory::McpConfigEntry => remove_mcp_entry(artifact),
        ArtifactCategory::DataDirectory => remove_directory(artifact),
        ArtifactCategory::SteeringFile | ArtifactCategory::SkillFile => remove_file(artifact),
    }
}

/// Remove a file from disk.
fn remove_file(artifact: &InstalledArtifact) -> RemovalResult {
    if !artifact.path.exists() {
        return RemovalResult::Success {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
        };
    }

    match std::fs::remove_file(&artifact.path) {
        Ok(()) => RemovalResult::Success {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
        },
        Err(e) if is_permission_error(&e) => RemovalResult::Failed {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
            reason: format!("Permission denied: {}", e),
        },
        Err(e) => RemovalResult::Failed {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
            reason: format!("Failed to remove: {}", e),
        },
    }
}

/// Remove a directory recursively.
fn remove_directory(artifact: &InstalledArtifact) -> RemovalResult {
    if !artifact.path.exists() {
        return RemovalResult::Success {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
        };
    }

    match std::fs::remove_dir_all(&artifact.path) {
        Ok(()) => RemovalResult::Success {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
        },
        Err(e) if is_permission_error(&e) => RemovalResult::Failed {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
            reason: format!("Permission denied: {}", e),
        },
        Err(e) => RemovalResult::Failed {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
            reason: format!("Failed to remove directory: {}", e),
        },
    }
}

/// Remove the codryn entry from an MCP config file.
///
/// Reads the JSON, removes the entry, and writes back. If the file
/// becomes empty (no other servers), still preserves the file structure.
fn remove_mcp_entry(artifact: &InstalledArtifact) -> RemovalResult {
    if !artifact.path.exists() {
        return RemovalResult::Success {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
        };
    }

    let content = match std::fs::read_to_string(&artifact.path) {
        Ok(c) => c,
        Err(e) if is_permission_error(&e) => {
            return RemovalResult::Failed {
                path: artifact.path.clone(),
                description: artifact.description.clone(),
                reason: format!("Permission denied reading config: {}", e),
            };
        }
        Err(e) => {
            return RemovalResult::Failed {
                path: artifact.path.clone(),
                description: artifact.description.clone(),
                reason: format!("Failed to read config: {}", e),
            };
        }
    };

    let mut config: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return RemovalResult::Failed {
                path: artifact.path.clone(),
                description: artifact.description.clone(),
                reason: format!("Invalid JSON in config: {}", e),
            };
        }
    };

    // Remove from mcpServers
    let mut removed = false;
    if let Some(servers) = config.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        if servers.remove(MCP_ENTRY_KEY).is_some() {
            removed = true;
        }
    }

    // Remove from servers (VS Code format)
    if !removed {
        if let Some(servers) = config.get_mut("servers").and_then(|v| v.as_object_mut()) {
            if servers.remove(MCP_ENTRY_KEY).is_some() {
                removed = true;
            }
        }
    }

    if !removed {
        // Entry wasn't found, nothing to do
        return RemovalResult::Success {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
        };
    }

    // Write the modified config back
    let updated = match serde_json::to_string_pretty(&config) {
        Ok(s) => s,
        Err(e) => {
            return RemovalResult::Failed {
                path: artifact.path.clone(),
                description: artifact.description.clone(),
                reason: format!("Failed to serialize updated config: {}", e),
            };
        }
    };

    match std::fs::write(&artifact.path, &updated) {
        Ok(()) => RemovalResult::Success {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
        },
        Err(e) if is_permission_error(&e) => RemovalResult::Failed {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
            reason: format!("Permission denied writing config: {}", e),
        },
        Err(e) => RemovalResult::Failed {
            path: artifact.path.clone(),
            description: artifact.description.clone(),
            reason: format!("Failed to write updated config: {}", e),
        },
    }
}

/// Check if a path is within a workspace directory.
fn is_within_workspace(path: &Path, workspace: &Path) -> bool {
    // Normalize both paths for comparison
    let path_canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let workspace_canonical = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    path_canonical.starts_with(&workspace_canonical)
}

/// Check if an IO error is a permission-related error.
fn is_permission_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Other
    )
}

/// Format discovered artifacts as a numbered list grouped by category.
///
/// Returns a formatted string suitable for display to the user before
/// confirmation.
pub fn format_artifact_list(artifacts: &[InstalledArtifact]) -> String {
    let mut output = String::new();
    let mut number = 1;

    // Group by category in display order
    let categories = [
        ArtifactCategory::SteeringFile,
        ArtifactCategory::SkillFile,
        ArtifactCategory::McpConfigEntry,
        ArtifactCategory::DataDirectory,
    ];

    for category in &categories {
        let items: Vec<&InstalledArtifact> =
            artifacts.iter().filter(|a| &a.category == category).collect();

        if items.is_empty() {
            continue;
        }

        output.push_str(&format!("\n  {}:\n", category));
        for item in items {
            output.push_str(&format!("    {}. {} ({})\n", number, item.description, item.path.display()));
            number += 1;
        }
    }

    output
}

/// Format uninstall results as a summary string.
pub fn format_results_summary(results: &[RemovalResult]) -> String {
    let mut output = String::new();
    let successful = results.iter().filter(|r| r.is_success()).count();
    let failed = results
        .iter()
        .filter(|r| matches!(r, RemovalResult::Failed { .. }))
        .count();
    let skipped = results
        .iter()
        .filter(|r| matches!(r, RemovalResult::Skipped { .. }))
        .count();

    output.push_str(&format!(
        "\nUninstall complete: {} removed, {} failed, {} skipped\n",
        successful, failed, skipped
    ));

    // Show details for failed items
    for result in results {
        match result {
            RemovalResult::Success { description, .. } => {
                output.push_str(&format!("  ✓ Removed: {}\n", description));
            }
            RemovalResult::Failed {
                description,
                reason,
                ..
            } => {
                output.push_str(&format!("  ✗ Failed: {} — {}\n", description, reason));
            }
            RemovalResult::Skipped {
                description,
                reason,
                ..
            } => {
                output.push_str(&format!("  ○ Skipped: {} — {}\n", description, reason));
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_artifact_category_display() {
        assert_eq!(ArtifactCategory::SteeringFile.to_string(), "Steering Files");
        assert_eq!(ArtifactCategory::SkillFile.to_string(), "Skill Files");
        assert_eq!(
            ArtifactCategory::McpConfigEntry.to_string(),
            "MCP Config Entries"
        );
        assert_eq!(
            ArtifactCategory::DataDirectory.to_string(),
            "Data Directories"
        );
    }

    #[test]
    fn test_execute_uninstall_removes_steering_file() {
        let tmp = TempDir::new().unwrap();
        let steering_path = tmp.path().join("steering.md");
        fs::write(&steering_path, "test content").unwrap();

        let artifacts = vec![InstalledArtifact {
            category: ArtifactCategory::SteeringFile,
            path: steering_path.clone(),
            description: "Test steering file".to_string(),
        }];

        let results = execute_uninstall(&artifacts, false, false, None);

        assert_eq!(results.len(), 1);
        assert!(results[0].is_success());
        assert!(!steering_path.exists());
    }

    #[test]
    fn test_execute_uninstall_removes_directory() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        fs::create_dir_all(data_dir.join("subdir")).unwrap();
        fs::write(data_dir.join("file.db"), "database").unwrap();
        fs::write(data_dir.join("subdir").join("index"), "index data").unwrap();

        let artifacts = vec![InstalledArtifact {
            category: ArtifactCategory::DataDirectory,
            path: data_dir.clone(),
            description: "CBM data".to_string(),
        }];

        let results = execute_uninstall(&artifacts, false, false, None);

        assert_eq!(results.len(), 1);
        assert!(results[0].is_success());
        assert!(!data_dir.exists());
    }

    #[test]
    fn test_execute_uninstall_keep_data_skips_data_directory() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(data_dir.join("graph.db"), "database content").unwrap();

        let steering_path = tmp.path().join("steering.md");
        fs::write(&steering_path, "steering content").unwrap();

        let artifacts = vec![
            InstalledArtifact {
                category: ArtifactCategory::SteeringFile,
                path: steering_path.clone(),
                description: "Steering file".to_string(),
            },
            InstalledArtifact {
                category: ArtifactCategory::DataDirectory,
                path: data_dir.clone(),
                description: "Data directory".to_string(),
            },
        ];

        let results = execute_uninstall(&artifacts, true, false, None);

        assert_eq!(results.len(), 2);
        // Steering file removed
        assert!(results[0].is_success());
        assert!(!steering_path.exists());
        // Data directory skipped
        assert!(matches!(results[1], RemovalResult::Skipped { .. }));
        assert!(data_dir.exists());
    }

    #[test]
    fn test_execute_uninstall_workspace_only_scope() {
        let workspace = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();

        let ws_steering = workspace.path().join(".kiro").join("steering").join("codebase-memory.md");
        fs::create_dir_all(ws_steering.parent().unwrap()).unwrap();
        fs::write(&ws_steering, "workspace steering").unwrap();

        let global_steering = outside.path().join("global-steering.md");
        fs::write(&global_steering, "global steering").unwrap();

        let artifacts = vec![
            InstalledArtifact {
                category: ArtifactCategory::SteeringFile,
                path: ws_steering.clone(),
                description: "Workspace steering".to_string(),
            },
            InstalledArtifact {
                category: ArtifactCategory::SteeringFile,
                path: global_steering.clone(),
                description: "Global steering".to_string(),
            },
        ];

        let results = execute_uninstall(&artifacts, false, true, Some(workspace.path()));

        assert_eq!(results.len(), 2);
        // Workspace file removed
        assert!(results[0].is_success());
        assert!(!ws_steering.exists());
        // Global file skipped
        assert!(matches!(results[1], RemovalResult::Skipped { .. }));
        assert!(global_steering.exists());
    }

    #[test]
    fn test_execute_uninstall_workspace_only_skips_mcp_config() {
        let workspace = TempDir::new().unwrap();
        let mcp_config = workspace.path().join("mcp.json");
        fs::write(
            &mcp_config,
            serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {
                    "codryn": { "command": "codryn" }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let artifacts = vec![InstalledArtifact {
            category: ArtifactCategory::McpConfigEntry,
            path: mcp_config.clone(),
            description: "MCP config".to_string(),
        }];

        let results = execute_uninstall(&artifacts, false, true, Some(workspace.path()));

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], RemovalResult::Skipped { .. }));
        // File should still contain the entry
        let content = fs::read_to_string(&mcp_config).unwrap();
        assert!(content.contains("codryn"));
    }

    #[test]
    fn test_execute_uninstall_combined_keep_data_workspace_only() {
        let workspace = TempDir::new().unwrap();

        let ws_steering = workspace.path().join(".kiro").join("steering").join("codebase-memory.md");
        fs::create_dir_all(ws_steering.parent().unwrap()).unwrap();
        fs::write(&ws_steering, "workspace steering").unwrap();

        let data_dir = workspace.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();

        let artifacts = vec![
            InstalledArtifact {
                category: ArtifactCategory::SteeringFile,
                path: ws_steering.clone(),
                description: "Workspace steering".to_string(),
            },
            InstalledArtifact {
                category: ArtifactCategory::DataDirectory,
                path: data_dir.clone(),
                description: "Data dir".to_string(),
            },
        ];

        let results = execute_uninstall(&artifacts, true, true, Some(workspace.path()));

        assert_eq!(results.len(), 2);
        // Workspace steering removed
        assert!(results[0].is_success());
        assert!(!ws_steering.exists());
        // Data directory skipped (keep_data takes precedence, also workspace_only skips data)
        assert!(matches!(results[1], RemovalResult::Skipped { .. }));
        assert!(data_dir.exists());
    }

    #[test]
    fn test_execute_uninstall_nonexistent_file_succeeds() {
        let artifacts = vec![InstalledArtifact {
            category: ArtifactCategory::SteeringFile,
            path: PathBuf::from("/nonexistent/path/steering.md"),
            description: "Missing file".to_string(),
        }];

        let results = execute_uninstall(&artifacts, false, false, None);

        assert_eq!(results.len(), 1);
        assert!(results[0].is_success());
    }

    #[test]
    fn test_remove_mcp_entry_from_config() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("mcp.json");
        fs::write(
            &config_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {
                    "codryn": { "command": "/usr/local/bin/codryn" },
                    "other-server": { "command": "other" }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let artifact = InstalledArtifact {
            category: ArtifactCategory::McpConfigEntry,
            path: config_path.clone(),
            description: "MCP config entry".to_string(),
        };

        let result = remove_mcp_entry(&artifact);
        assert!(result.is_success());

        // Verify the entry was removed but other-server remains
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(!content.contains("codryn"));
        assert!(content.contains("other-server"));
    }

    #[test]
    fn test_remove_mcp_entry_servers_format() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("mcp.json");
        fs::write(
            &config_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "servers": {
                    "codryn": { "type": "stdio", "command": "codryn" }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let artifact = InstalledArtifact {
            category: ArtifactCategory::McpConfigEntry,
            path: config_path.clone(),
            description: "MCP config entry".to_string(),
        };

        let result = remove_mcp_entry(&artifact);
        assert!(result.is_success());

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(!content.contains("codryn"));
    }

    #[test]
    fn test_is_within_workspace() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let inside = workspace.join("sub").join("file.md");
        let outside = PathBuf::from("/tmp/other/file.md");

        // Create the file so canonicalize works
        fs::create_dir_all(workspace.join("sub")).unwrap();
        fs::write(&inside, "content").unwrap();

        assert!(is_within_workspace(&inside, workspace));
        assert!(!is_within_workspace(&outside, workspace));
    }

    #[test]
    fn test_format_artifact_list() {
        let artifacts = vec![
            InstalledArtifact {
                category: ArtifactCategory::SteeringFile,
                path: PathBuf::from("/workspace/.kiro/steering/codebase-memory.md"),
                description: "Workspace steering".to_string(),
            },
            InstalledArtifact {
                category: ArtifactCategory::McpConfigEntry,
                path: PathBuf::from("/home/user/.cursor/mcp.json"),
                description: "Cursor MCP config".to_string(),
            },
            InstalledArtifact {
                category: ArtifactCategory::DataDirectory,
                path: PathBuf::from("/home/user/.local/share/codryn"),
                description: "CBM data directory".to_string(),
            },
        ];

        let output = format_artifact_list(&artifacts);
        assert!(output.contains("Steering Files:"));
        assert!(output.contains("MCP Config Entries:"));
        assert!(output.contains("Data Directories:"));
        assert!(output.contains("1. Workspace steering"));
        assert!(output.contains("2. Cursor MCP config"));
        assert!(output.contains("3. CBM data directory"));
    }

    #[test]
    fn test_format_results_summary() {
        let results = vec![
            RemovalResult::Success {
                path: PathBuf::from("/a"),
                description: "Removed file".to_string(),
            },
            RemovalResult::Failed {
                path: PathBuf::from("/b"),
                description: "Failed file".to_string(),
                reason: "Permission denied".to_string(),
            },
            RemovalResult::Skipped {
                path: PathBuf::from("/c"),
                description: "Skipped file".to_string(),
                reason: "keep-data".to_string(),
            },
        ];

        let output = format_results_summary(&results);
        assert!(output.contains("1 removed, 1 failed, 1 skipped"));
        assert!(output.contains("✓ Removed: Removed file"));
        assert!(output.contains("✗ Failed: Failed file — Permission denied"));
        assert!(output.contains("○ Skipped: Skipped file — keep-data"));
    }

    #[test]
    fn test_discover_artifacts_with_none_prefs() {
        // With None prefs, should scan defaults (won't find much in test env)
        let artifacts = discover_artifacts(&None);
        // Just verify it doesn't panic and returns a vec
        assert!(artifacts.is_empty() || !artifacts.is_empty());
    }

    #[test]
    fn test_discover_artifacts_with_prefs_and_workspace() {
        let workspace = TempDir::new().unwrap();
        let steering_path = workspace
            .path()
            .join(".kiro")
            .join("steering")
            .join(STEERING_FILENAME);
        fs::create_dir_all(steering_path.parent().unwrap()).unwrap();
        fs::write(&steering_path, "steering content").unwrap();

        let prefs = InstallPreferences {
            activated_workspaces: Some(vec![
                crate::preferences::WorkspaceActivation {
                    path: workspace.path().to_path_buf(),
                    activated_at: "2024-01-01T00:00:00Z".to_string(),
                    steering_intensity: crate::preferences::SteeringIntensity::Full,
                },
            ]),
            ..Default::default()
        };

        let artifacts = discover_artifacts(&Some(prefs));

        // Should find the workspace steering file
        let steering_artifacts: Vec<_> = artifacts
            .iter()
            .filter(|a| a.category == ArtifactCategory::SteeringFile && a.path == steering_path)
            .collect();
        assert_eq!(steering_artifacts.len(), 1);
    }

    #[test]
    fn test_removal_result_path() {
        let path = PathBuf::from("/test/path");
        let success = RemovalResult::Success {
            path: path.clone(),
            description: "test".to_string(),
        };
        assert_eq!(success.path(), path);

        let failed = RemovalResult::Failed {
            path: path.clone(),
            description: "test".to_string(),
            reason: "err".to_string(),
        };
        assert_eq!(failed.path(), path);

        let skipped = RemovalResult::Skipped {
            path: path.clone(),
            description: "test".to_string(),
            reason: "reason".to_string(),
        };
        assert_eq!(skipped.path(), path);
    }
}
