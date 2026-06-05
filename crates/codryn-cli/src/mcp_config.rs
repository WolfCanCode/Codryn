//! MCP configuration file management.
//!
//! Provides `McpConfigManager` for discovering, adding, removing, and modifying
//! `codryn` entries in IDE MCP configuration files. Supports
//! before/after diffs for user confirmation, deep merge of existing user-modified
//! values, and graceful handling of invalid JSON and non-existent files.

use anyhow::Result;
use std::path::{Path, PathBuf};

use codryn_foundation::ide_detect::{detect_ides, DetectedIde};

use crate::prompter::Prompter;

/// The key used to identify our MCP server entry in config files.
const MCP_ENTRY_KEY: &str = "codryn";

/// A discovered MCP config entry for codryn.
#[derive(Debug, Clone)]
pub struct McpConfigEntry {
    /// Path to the config file containing this entry.
    pub config_path: PathBuf,
    /// The IDE this config belongs to.
    pub ide_name: String,
    /// The JSON value of the entry (the server config object).
    pub entry: serde_json::Value,
}

/// A before/after diff for a proposed config file change.
#[derive(Debug, Clone)]
pub struct ConfigDiff {
    /// Path to the config file.
    pub path: PathBuf,
    /// Content before the change (empty string if file doesn't exist).
    pub before: String,
    /// Content after the proposed change.
    pub after: String,
    /// Whether this is a new file (doesn't exist yet).
    pub is_new_file: bool,
}

/// Operations on MCP configuration files.
///
/// Provides interactive management of `codryn` entries across
/// all detected IDE config files. All mutating operations confirm with the
/// user via the `Prompter` trait before writing.
pub struct McpConfigManager<'a> {
    prompter: &'a dyn Prompter,
}

impl<'a> McpConfigManager<'a> {
    /// Create a new manager with the given prompter for user interaction.
    pub fn new(prompter: &'a dyn Prompter) -> Self {
        Self { prompter }
    }

    /// Show all codryn entries across known config files.
    ///
    /// Scans all detected IDE MCP config paths and returns entries found.
    /// Invalid JSON files are reported via `prompter.info()` and skipped.
    pub fn show_all(&self) -> Result<Vec<McpConfigEntry>> {
        let detected_ides = detect_ides();
        self.show_all_with_ides(&detected_ides)
    }

    /// Internal implementation that accepts pre-detected IDEs (for testing).
    pub fn show_all_with_ides(&self, detected_ides: &[DetectedIde]) -> Result<Vec<McpConfigEntry>> {
        let mut entries = Vec::new();

        for ide in detected_ides {
            let config_path = &ide.mcp_config_path;
            if !config_path.exists() {
                continue;
            }

            let content = match std::fs::read_to_string(config_path) {
                Ok(c) => c,
                Err(e) => {
                    self.prompter.info(&format!(
                        "Warning: cannot read {}: {}",
                        config_path.display(),
                        e
                    ));
                    continue;
                }
            };

            let config: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    self.prompter.info(&format!(
                        "Error: invalid JSON in {}: {}",
                        config_path.display(),
                        e
                    ));
                    continue;
                }
            };

            // Look for the entry in known server object keys
            if let Some(entry) = find_mcp_entry(&config) {
                entries.push(McpConfigEntry {
                    config_path: config_path.clone(),
                    ide_name: ide.ide.display_name().to_string(),
                    entry: entry.clone(),
                });
            }
        }

        Ok(entries)
    }

    /// Add entry to specified config files with confirmation.
    ///
    /// For each target path, proposes changes with a diff, confirms with the user,
    /// and writes if approved. Non-existent files are created after showing full
    /// proposed content and receiving confirmation.
    pub fn add(&self, binary_path: &Path, targets: &[PathBuf]) -> Result<()> {
        for target in targets {
            let diff = match self.propose_changes(target, binary_path) {
                Ok(d) => d,
                Err(e) => {
                    self.prompter.info(&format!(
                        "Error: cannot propose changes for {}: {}",
                        target.display(),
                        e
                    ));
                    continue;
                }
            };

            if diff.is_new_file {
                self.prompter.info(&format!(
                    "File does not exist: {}",
                    diff.path.display()
                ));
                self.prompter.info("Proposed content:");
                self.prompter.info(&diff.after);
            } else {
                self.prompter
                    .show_diff(&diff.path.display().to_string(), &diff.before, &diff.after);
            }

            let confirmed = self.prompter.confirm(
                &format!("Apply changes to {}?", diff.path.display()),
                true,
            )?;

            if confirmed {
                if let Some(parent) = diff.path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&diff.path, &diff.after)?;
                self.prompter.info(&format!("Updated: {}", diff.path.display()));
            } else {
                self.prompter.info(&format!("Skipped: {}", diff.path.display()));
            }
        }

        Ok(())
    }

    /// Remove entry from specified config files with confirmation.
    ///
    /// For each target path, proposes removal, confirms with the user,
    /// and writes if approved. Files that don't exist or don't contain
    /// the entry are skipped.
    pub fn remove(&self, targets: &[PathBuf]) -> Result<()> {
        for target in targets {
            if !target.exists() {
                self.prompter.info(&format!(
                    "Skipped: {} (file does not exist)",
                    target.display()
                ));
                continue;
            }

            let content = match std::fs::read_to_string(target) {
                Ok(c) => c,
                Err(e) => {
                    self.prompter.info(&format!(
                        "Error: cannot read {}: {}",
                        target.display(),
                        e
                    ));
                    continue;
                }
            };

            let mut config: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    self.prompter.info(&format!(
                        "Error: invalid JSON in {}: {}",
                        target.display(),
                        e
                    ));
                    continue;
                }
            };

            let removed = remove_mcp_entry_from_value(&mut config);
            if !removed {
                self.prompter.info(&format!(
                    "Skipped: {} (no codryn entry found)",
                    target.display()
                ));
                continue;
            }

            let after = serde_json::to_string_pretty(&config)
                .unwrap_or_else(|_| "{}".to_string());

            self.prompter
                .show_diff(&target.display().to_string(), &content, &after);

            let confirmed = self.prompter.confirm(
                &format!("Remove codryn entry from {}?", target.display()),
                true,
            )?;

            if confirmed {
                std::fs::write(target, &after)?;
                self.prompter.info(&format!("Removed entry from: {}", target.display()));
            } else {
                self.prompter.info(&format!("Skipped: {}", target.display()));
            }
        }

        Ok(())
    }

    /// Propose changes and return diff for confirmation.
    ///
    /// Generates a `ConfigDiff` showing what would change if the codryn
    /// entry is added/updated in the given config file. For non-existent files,
    /// `is_new_file` is true and `before` is empty.
    pub fn propose_changes(&self, config_path: &Path, binary_path: &Path) -> Result<ConfigDiff> {
        let binary_str = binary_path.to_string_lossy().to_string();

        let proposed_entry = serde_json::json!({
            "command": binary_str,
            "args": []
        });

        if !config_path.exists() {
            // New file: create a minimal config with just the MCP entry
            let new_config = serde_json::json!({
                "mcpServers": {
                    MCP_ENTRY_KEY: proposed_entry
                }
            });
            let after = serde_json::to_string_pretty(&new_config)?;

            return Ok(ConfigDiff {
                path: config_path.to_path_buf(),
                before: String::new(),
                after,
                is_new_file: true,
            });
        }

        let before = std::fs::read_to_string(config_path)?;
        let mut config: serde_json::Value = serde_json::from_str(&before)
            .map_err(|e| anyhow::anyhow!("Invalid JSON in {}: {}", config_path.display(), e))?;

        // Determine which key to use for the servers object
        let servers_key = if config.get("servers").is_some() {
            "servers"
        } else {
            "mcpServers"
        };

        // Ensure the servers object exists
        if config.get(servers_key).is_none() {
            config
                .as_object_mut()
                .unwrap()
                .insert(servers_key.to_string(), serde_json::json!({}));
        }

        // Get or create the entry, merging if it already exists
        if let Some(servers) = config.get_mut(servers_key).and_then(|v| v.as_object_mut()) {
            if let Some(existing) = servers.get(MCP_ENTRY_KEY) {
                let merged = Self::merge_entry(existing, &proposed_entry);
                servers.insert(MCP_ENTRY_KEY.to_string(), merged);
            } else {
                servers.insert(MCP_ENTRY_KEY.to_string(), proposed_entry);
            }
        }

        let after = serde_json::to_string_pretty(&config)?;

        Ok(ConfigDiff {
            path: config_path.to_path_buf(),
            before,
            after,
            is_new_file: false,
        })
    }

    /// Merge new fields while preserving existing user-modified values.
    ///
    /// Deep merges `proposed` into `existing`: existing keys are preserved,
    /// new keys from proposed are added. For nested objects, the merge is recursive.
    pub fn merge_entry(
        existing: &serde_json::Value,
        proposed: &serde_json::Value,
    ) -> serde_json::Value {
        match (existing, proposed) {
            (serde_json::Value::Object(existing_map), serde_json::Value::Object(proposed_map)) => {
                let mut result = existing_map.clone();
                for (key, proposed_value) in proposed_map {
                    if let Some(existing_value) = result.get(key) {
                        // Recursively merge nested objects
                        if existing_value.is_object() && proposed_value.is_object() {
                            let merged = Self::merge_entry(existing_value, proposed_value);
                            result.insert(key.clone(), merged);
                        }
                        // Otherwise preserve existing value (don't overwrite)
                    } else {
                        // Add new field from proposed
                        result.insert(key.clone(), proposed_value.clone());
                    }
                }
                serde_json::Value::Object(result)
            }
            // Non-object values: preserve existing
            _ => existing.clone(),
        }
    }
}

// ── Helper functions ─────────────────────────────────────────────────────────

/// Find the codryn entry in a parsed config value.
///
/// Checks both `mcpServers` and `servers` keys (VS Code uses `servers`).
fn find_mcp_entry(config: &serde_json::Value) -> Option<&serde_json::Value> {
    // Try mcpServers first (most common)
    if let Some(servers) = config.get("mcpServers").and_then(|v| v.as_object()) {
        if let Some(entry) = servers.get(MCP_ENTRY_KEY) {
            return Some(entry);
        }
    }

    // Try servers (VS Code format)
    if let Some(servers) = config.get("servers").and_then(|v| v.as_object()) {
        if let Some(entry) = servers.get(MCP_ENTRY_KEY) {
            return Some(entry);
        }
    }

    None
}

/// Remove the codryn entry from a parsed config value.
///
/// Returns true if an entry was found and removed.
fn remove_mcp_entry_from_value(config: &mut serde_json::Value) -> bool {
    // Try mcpServers first
    if let Some(servers) = config.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        if servers.remove(MCP_ENTRY_KEY).is_some() {
            return true;
        }
    }

    // Try servers (VS Code format)
    if let Some(servers) = config.get_mut("servers").and_then(|v| v.as_object_mut()) {
        if servers.remove(MCP_ENTRY_KEY).is_some() {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompter::{MockPrompter, MockResponse};
    use tempfile::TempDir;

    fn setup_config_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_merge_entry_preserves_existing_values() {
        let existing = serde_json::json!({
            "command": "/usr/local/bin/codryn",
            "args": ["--verbose"],
            "env": { "LOG_LEVEL": "debug" },
            "autoApprove": ["index_repository"]
        });

        let proposed = serde_json::json!({
            "command": "/new/path/codryn",
            "args": [],
            "disabled": false
        });

        let merged = McpConfigManager::merge_entry(&existing, &proposed);

        // Existing values preserved
        assert_eq!(merged["command"], "/usr/local/bin/codryn");
        assert_eq!(merged["args"], serde_json::json!(["--verbose"]));
        assert_eq!(merged["env"]["LOG_LEVEL"], "debug");
        assert_eq!(
            merged["autoApprove"],
            serde_json::json!(["index_repository"])
        );
        // New field added
        assert_eq!(merged["disabled"], false);
    }

    #[test]
    fn test_merge_entry_adds_all_new_fields() {
        let existing = serde_json::json!({
            "command": "/usr/local/bin/codryn"
        });

        let proposed = serde_json::json!({
            "command": "/new/path/codryn",
            "args": [],
            "env": { "DEBUG": "1" }
        });

        let merged = McpConfigManager::merge_entry(&existing, &proposed);

        assert_eq!(merged["command"], "/usr/local/bin/codryn");
        assert_eq!(merged["args"], serde_json::json!([]));
        assert_eq!(merged["env"]["DEBUG"], "1");
    }

    #[test]
    fn test_merge_entry_deep_merges_nested_objects() {
        let existing = serde_json::json!({
            "command": "/usr/local/bin/codryn",
            "env": { "LOG_LEVEL": "debug", "USER_SETTING": "custom" }
        });

        let proposed = serde_json::json!({
            "command": "/new/path/codryn",
            "env": { "LOG_LEVEL": "info", "NEW_VAR": "value" }
        });

        let merged = McpConfigManager::merge_entry(&existing, &proposed);

        // Existing nested values preserved
        assert_eq!(merged["env"]["LOG_LEVEL"], "debug");
        assert_eq!(merged["env"]["USER_SETTING"], "custom");
        // New nested values added
        assert_eq!(merged["env"]["NEW_VAR"], "value");
    }

    #[test]
    fn test_merge_entry_non_object_preserved() {
        let existing = serde_json::json!("string_value");
        let proposed = serde_json::json!({ "key": "value" });

        let merged = McpConfigManager::merge_entry(&existing, &proposed);
        assert_eq!(merged, serde_json::json!("string_value"));
    }

    #[test]
    fn test_show_all_with_valid_config() {
        let tmp = TempDir::new().unwrap();
        let config_path = setup_config_file(
            &tmp,
            "mcp.json",
            &serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {
                    "codryn": {
                        "command": "/usr/local/bin/codryn"
                    }
                }
            }))
            .unwrap(),
        );

        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        let ides = vec![DetectedIde {
            ide: codryn_foundation::ide_detect::Ide::Cursor,
            config_dir: tmp.path().to_path_buf(),
            mcp_config_path: config_path,
            detection_method: "directory",
        }];

        let entries = manager.show_all_with_ides(&ides).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ide_name, "Cursor");
        assert_eq!(entries[0].entry["command"], "/usr/local/bin/codryn");
    }

    #[test]
    fn test_show_all_skips_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let config_path = setup_config_file(&tmp, "mcp.json", "not valid json {{{");

        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        let ides = vec![DetectedIde {
            ide: codryn_foundation::ide_detect::Ide::Cursor,
            config_dir: tmp.path().to_path_buf(),
            mcp_config_path: config_path,
            detection_method: "directory",
        }];

        let entries = manager.show_all_with_ides(&ides).unwrap();
        assert_eq!(entries.len(), 0);

        // Should have reported the error via info()
        let history = prompter.call_history();
        assert!(history.iter().any(|call| {
            if let crate::prompter::PromptCall::Info { message } = call {
                message.contains("invalid JSON")
            } else {
                false
            }
        }));
    }

    #[test]
    fn test_show_all_skips_nonexistent_files() {
        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        let ides = vec![DetectedIde {
            ide: codryn_foundation::ide_detect::Ide::Cursor,
            config_dir: PathBuf::from("/nonexistent"),
            mcp_config_path: PathBuf::from("/nonexistent/mcp.json"),
            detection_method: "directory",
        }];

        let entries = manager.show_all_with_ides(&ides).unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_show_all_finds_vscode_servers_format() {
        let tmp = TempDir::new().unwrap();
        let config_path = setup_config_file(
            &tmp,
            "mcp.json",
            &serde_json::to_string_pretty(&serde_json::json!({
                "servers": {
                    "codryn": {
                        "type": "stdio",
                        "command": "/usr/local/bin/codryn"
                    }
                }
            }))
            .unwrap(),
        );

        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        let ides = vec![DetectedIde {
            ide: codryn_foundation::ide_detect::Ide::VsCode,
            config_dir: tmp.path().to_path_buf(),
            mcp_config_path: config_path,
            detection_method: "directory",
        }];

        let entries = manager.show_all_with_ides(&ides).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ide_name, "VS Code");
    }

    #[test]
    fn test_propose_changes_new_file() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("new_mcp.json");

        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        let diff = manager
            .propose_changes(&config_path, Path::new("/usr/local/bin/codryn"))
            .unwrap();

        assert!(diff.is_new_file);
        assert!(diff.before.is_empty());
        assert!(diff.after.contains("codryn"));
        assert!(diff.after.contains("/usr/local/bin/codryn"));
    }

    #[test]
    fn test_propose_changes_existing_file_no_entry() {
        let tmp = TempDir::new().unwrap();
        let config_path = setup_config_file(
            &tmp,
            "mcp.json",
            &serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {
                    "other-server": { "command": "other" }
                }
            }))
            .unwrap(),
        );

        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        let diff = manager
            .propose_changes(&config_path, Path::new("/usr/local/bin/codryn"))
            .unwrap();

        assert!(!diff.is_new_file);
        assert!(!diff.before.contains("codryn"));
        assert!(diff.after.contains("codryn"));
        assert!(diff.after.contains("other-server"));
    }

    #[test]
    fn test_propose_changes_existing_entry_merges() {
        let tmp = TempDir::new().unwrap();
        let config_path = setup_config_file(
            &tmp,
            "mcp.json",
            &serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {
                    "codryn": {
                        "command": "/old/path/codryn",
                        "env": { "CUSTOM": "value" }
                    }
                }
            }))
            .unwrap(),
        );

        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        let diff = manager
            .propose_changes(&config_path, Path::new("/new/path/codryn"))
            .unwrap();

        assert!(!diff.is_new_file);
        // Existing command preserved (merge preserves existing values)
        assert!(diff.after.contains("/old/path/codryn"));
        // Custom env preserved
        assert!(diff.after.contains("CUSTOM"));
        // New args field added
        assert!(diff.after.contains("args"));
    }

    #[test]
    fn test_propose_changes_invalid_json_returns_error() {
        let tmp = TempDir::new().unwrap();
        let config_path = setup_config_file(&tmp, "mcp.json", "not json {{{");

        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        let result = manager.propose_changes(&config_path, Path::new("/usr/local/bin/codryn"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid JSON"));
    }

    #[test]
    fn test_add_creates_new_file_with_confirmation() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("new_mcp.json");

        let prompter = MockPrompter::new(vec![MockResponse::Confirm(true)]);
        let manager = McpConfigManager::new(&prompter);

        manager
            .add(Path::new("/usr/local/bin/codryn"), &[config_path.clone()])
            .unwrap();

        assert!(config_path.exists());
        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("codryn"));
        assert!(content.contains("/usr/local/bin/codryn"));
    }

    #[test]
    fn test_add_skips_on_rejection() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("new_mcp.json");

        let prompter = MockPrompter::new(vec![MockResponse::Confirm(false)]);
        let manager = McpConfigManager::new(&prompter);

        manager
            .add(Path::new("/usr/local/bin/codryn"), &[config_path.clone()])
            .unwrap();

        assert!(!config_path.exists());
    }

    #[test]
    fn test_add_handles_multiple_targets() {
        let tmp = TempDir::new().unwrap();
        let path1 = tmp.path().join("config1.json");
        let path2 = tmp.path().join("config2.json");

        let prompter = MockPrompter::new(vec![
            MockResponse::Confirm(true),
            MockResponse::Confirm(false),
        ]);
        let manager = McpConfigManager::new(&prompter);

        manager
            .add(Path::new("/usr/local/bin/codryn"), &[path1.clone(), path2.clone()])
            .unwrap();

        assert!(path1.exists());
        assert!(!path2.exists());
    }

    #[test]
    fn test_remove_removes_entry_with_confirmation() {
        let tmp = TempDir::new().unwrap();
        let config_path = setup_config_file(
            &tmp,
            "mcp.json",
            &serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {
                    "codryn": { "command": "/usr/local/bin/codryn" },
                    "other-server": { "command": "other" }
                }
            }))
            .unwrap(),
        );

        let prompter = MockPrompter::new(vec![MockResponse::Confirm(true)]);
        let manager = McpConfigManager::new(&prompter);

        manager.remove(&[config_path.clone()]).unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(!content.contains("codryn"));
        assert!(content.contains("other-server"));
    }

    #[test]
    fn test_remove_skips_on_rejection() {
        let tmp = TempDir::new().unwrap();
        let original_content = serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": {
                "codryn": { "command": "/usr/local/bin/codryn" }
            }
        }))
        .unwrap();
        let config_path = setup_config_file(&tmp, "mcp.json", &original_content);

        let prompter = MockPrompter::new(vec![MockResponse::Confirm(false)]);
        let manager = McpConfigManager::new(&prompter);

        manager.remove(&[config_path.clone()]).unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("codryn"));
    }

    #[test]
    fn test_remove_skips_nonexistent_file() {
        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        // Should not error, and should not consume any prompt responses
        manager
            .remove(&[PathBuf::from("/nonexistent/mcp.json")])
            .unwrap();
        assert_eq!(prompter.remaining_responses(), 0);
    }

    #[test]
    fn test_remove_skips_file_without_entry() {
        let tmp = TempDir::new().unwrap();
        let config_path = setup_config_file(
            &tmp,
            "mcp.json",
            &serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {
                    "other-server": { "command": "other" }
                }
            }))
            .unwrap(),
        );

        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        manager.remove(&[config_path]).unwrap();
        // No confirm should be called since entry wasn't found
        assert_eq!(prompter.remaining_responses(), 0);
    }

    #[test]
    fn test_remove_handles_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let config_path = setup_config_file(&tmp, "mcp.json", "invalid json {{{");

        let prompter = MockPrompter::new(vec![]);
        let manager = McpConfigManager::new(&prompter);

        // Should not error — just skip the file
        manager.remove(&[config_path]).unwrap();

        let history = prompter.call_history();
        assert!(history.iter().any(|call| {
            if let crate::prompter::PromptCall::Info { message } = call {
                message.contains("invalid JSON")
            } else {
                false
            }
        }));
    }

    #[test]
    fn test_find_mcp_entry_in_mcp_servers() {
        let config = serde_json::json!({
            "mcpServers": {
                "codryn": { "command": "codryn" }
            }
        });
        let entry = find_mcp_entry(&config);
        assert!(entry.is_some());
        assert_eq!(entry.unwrap()["command"], "codryn");
    }

    #[test]
    fn test_find_mcp_entry_in_servers() {
        let config = serde_json::json!({
            "servers": {
                "codryn": { "type": "stdio", "command": "codryn" }
            }
        });
        let entry = find_mcp_entry(&config);
        assert!(entry.is_some());
        assert_eq!(entry.unwrap()["command"], "codryn");
    }

    #[test]
    fn test_find_mcp_entry_not_found() {
        let config = serde_json::json!({
            "mcpServers": {
                "other-server": { "command": "other" }
            }
        });
        assert!(find_mcp_entry(&config).is_none());
    }

    #[test]
    fn test_remove_mcp_entry_from_value_mcp_servers() {
        let mut config = serde_json::json!({
            "mcpServers": {
                "codryn": { "command": "codryn" },
                "other": { "command": "other" }
            }
        });
        assert!(remove_mcp_entry_from_value(&mut config));
        assert!(config["mcpServers"].get("codryn").is_none());
        assert!(config["mcpServers"].get("other").is_some());
    }

    #[test]
    fn test_remove_mcp_entry_from_value_servers() {
        let mut config = serde_json::json!({
            "servers": {
                "codryn": { "command": "codryn" }
            }
        });
        assert!(remove_mcp_entry_from_value(&mut config));
        assert!(config["servers"].get("codryn").is_none());
    }

    #[test]
    fn test_remove_mcp_entry_from_value_not_found() {
        let mut config = serde_json::json!({
            "mcpServers": {
                "other": { "command": "other" }
            }
        });
        assert!(!remove_mcp_entry_from_value(&mut config));
    }
}
