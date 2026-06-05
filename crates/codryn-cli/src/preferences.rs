use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Install scope: where artifacts are placed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum InstallScope {
    Global,
    WorkspaceOnly,
    Both,
}

/// Steering intensity level
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SteeringIntensity {
    Full,
    Lite,
    None,
}

/// Steering installation preference
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SteeringChoice {
    Yes,
    No,
    WorkspaceOnly,
}

/// Per-workspace activation record
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct WorkspaceActivation {
    pub path: PathBuf,
    pub activated_at: String, // ISO 8601
    pub steering_intensity: SteeringIntensity,
}

/// Persisted user preferences for the install flow.
///
/// Stored at `~/.config/codryn/install-preferences.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct InstallPreferences {
    pub scope: Option<InstallScope>,
    pub steering: Option<SteeringChoice>,
    pub global_intensity: Option<SteeringIntensity>,
    pub workspace_intensity: Option<SteeringIntensity>,
    pub selected_ides: Option<Vec<String>>,
    pub activated_workspaces: Option<Vec<WorkspaceActivation>>,
}

impl InstallPreferences {
    /// Load preferences from `~/.config/codryn/install-preferences.toml`.
    ///
    /// Returns `Ok(Default)` if the file does not exist.
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load() -> Result<Self, anyhow::Error> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to read preferences file at {}: {}",
                path.display(),
                e
            )
        })?;
        let prefs: Self = toml::from_str(&contents).map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse preferences file at {}: {}",
                path.display(),
                e
            )
        })?;
        Ok(prefs)
    }

    /// Save preferences to `~/.config/codryn/install-preferences.toml`.
    ///
    /// Creates parent directories if they don't exist.
    /// Returns an error if the file cannot be written (permission denied, disk full, etc).
    pub fn save(&self) -> Result<(), anyhow::Error> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create preferences directory at {}: {}",
                    parent.display(),
                    e
                )
            })?;
        }
        let contents = toml::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize preferences: {}", e))?;
        std::fs::write(&path, &contents).map_err(|e| {
            anyhow::anyhow!(
                "Failed to write preferences file at {}: {}",
                path.display(),
                e
            )
        })?;
        Ok(())
    }

    /// Returns the path to the preferences file: `~/.config/codryn/install-preferences.toml`
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("codryn")
            .join("install-preferences.toml")
    }

    /// Get effective scope, defaulting to `WorkspaceOnly` if not set.
    pub fn effective_scope(&self) -> InstallScope {
        self.scope.clone().unwrap_or(InstallScope::WorkspaceOnly)
    }

    /// Get effective steering intensity for a given scope.
    ///
    /// Defaults:
    /// - Global: `Lite`
    /// - WorkspaceOnly: `Full`
    /// - Both: `Full` (uses workspace intensity)
    pub fn effective_intensity(&self, scope: &InstallScope) -> SteeringIntensity {
        match scope {
            InstallScope::Global => self
                .global_intensity
                .clone()
                .unwrap_or(SteeringIntensity::Lite),
            InstallScope::WorkspaceOnly => self
                .workspace_intensity
                .clone()
                .unwrap_or(SteeringIntensity::Full),
            InstallScope::Both => self
                .workspace_intensity
                .clone()
                .unwrap_or(SteeringIntensity::Full),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_default_preferences() {
        let prefs = InstallPreferences::default();
        assert_eq!(prefs.scope, None);
        assert_eq!(prefs.steering, None);
        assert_eq!(prefs.global_intensity, None);
        assert_eq!(prefs.workspace_intensity, None);
        assert_eq!(prefs.selected_ides, None);
        assert_eq!(prefs.activated_workspaces, None);
    }

    #[test]
    fn test_effective_scope_default() {
        let prefs = InstallPreferences::default();
        assert_eq!(prefs.effective_scope(), InstallScope::WorkspaceOnly);
    }

    #[test]
    fn test_effective_scope_set() {
        let prefs = InstallPreferences {
            scope: Some(InstallScope::Global),
            ..Default::default()
        };
        assert_eq!(prefs.effective_scope(), InstallScope::Global);
    }

    #[test]
    fn test_effective_intensity_global_default() {
        let prefs = InstallPreferences::default();
        assert_eq!(
            prefs.effective_intensity(&InstallScope::Global),
            SteeringIntensity::Lite
        );
    }

    #[test]
    fn test_effective_intensity_workspace_default() {
        let prefs = InstallPreferences::default();
        assert_eq!(
            prefs.effective_intensity(&InstallScope::WorkspaceOnly),
            SteeringIntensity::Full
        );
    }

    #[test]
    fn test_effective_intensity_both_default() {
        let prefs = InstallPreferences::default();
        assert_eq!(
            prefs.effective_intensity(&InstallScope::Both),
            SteeringIntensity::Full
        );
    }

    #[test]
    fn test_effective_intensity_custom() {
        let prefs = InstallPreferences {
            global_intensity: Some(SteeringIntensity::Full),
            workspace_intensity: Some(SteeringIntensity::None),
            ..Default::default()
        };
        assert_eq!(
            prefs.effective_intensity(&InstallScope::Global),
            SteeringIntensity::Full
        );
        assert_eq!(
            prefs.effective_intensity(&InstallScope::WorkspaceOnly),
            SteeringIntensity::None
        );
        assert_eq!(
            prefs.effective_intensity(&InstallScope::Both),
            SteeringIntensity::None
        );
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let prefs = InstallPreferences {
            scope: Some(InstallScope::WorkspaceOnly),
            steering: Some(SteeringChoice::WorkspaceOnly),
            global_intensity: Some(SteeringIntensity::Lite),
            workspace_intensity: Some(SteeringIntensity::Full),
            selected_ides: Some(vec!["cursor".to_string(), "kiro".to_string()]),
            activated_workspaces: Some(vec![WorkspaceActivation {
                path: PathBuf::from("/home/user/projects/my-app"),
                activated_at: "2024-12-01T10:30:00Z".to_string(),
                steering_intensity: SteeringIntensity::Full,
            }]),
        };

        let toml_str = toml::to_string_pretty(&prefs).unwrap();
        let loaded: InstallPreferences = toml::from_str(&toml_str).unwrap();

        assert_eq!(loaded.scope, prefs.scope);
        assert_eq!(loaded.steering, prefs.steering);
        assert_eq!(loaded.global_intensity, prefs.global_intensity);
        assert_eq!(loaded.workspace_intensity, prefs.workspace_intensity);
        assert_eq!(loaded.selected_ides, prefs.selected_ides);
    }

    #[test]
    fn test_toml_format_matches_spec() {
        // Test that the TOML output matches the format from the design doc
        let prefs = InstallPreferences {
            scope: Some(InstallScope::WorkspaceOnly),
            steering: Some(SteeringChoice::WorkspaceOnly),
            global_intensity: Some(SteeringIntensity::Lite),
            workspace_intensity: Some(SteeringIntensity::Full),
            selected_ides: Some(vec!["cursor".to_string(), "kiro".to_string()]),
            activated_workspaces: Some(vec![WorkspaceActivation {
                path: PathBuf::from("/home/user/projects/my-app"),
                activated_at: "2024-12-01T10:30:00Z".to_string(),
                steering_intensity: SteeringIntensity::Full,
            }]),
        };

        let toml_str = toml::to_string_pretty(&prefs).unwrap();
        assert!(toml_str.contains("scope = \"workspace-only\""));
        assert!(toml_str.contains("steering = \"workspace-only\""));
        assert!(toml_str.contains("global-intensity = \"lite\""));
        assert!(toml_str.contains("workspace-intensity = \"full\""));
        assert!(toml_str.contains("selected-ides"));
        assert!(toml_str.contains("cursor"));
        assert!(toml_str.contains("kiro"));
    }

    #[test]
    fn test_load_from_toml_string() {
        let toml_content = r#"
scope = "workspace-only"
steering = "workspace-only"
global-intensity = "lite"
workspace-intensity = "full"
selected-ides = ["cursor", "kiro"]

[[activated-workspaces]]
path = "/home/user/projects/my-app"
activated-at = "2024-12-01T10:30:00Z"
steering-intensity = "full"
"#;
        let prefs: InstallPreferences = toml::from_str(toml_content).unwrap();
        assert_eq!(prefs.scope, Some(InstallScope::WorkspaceOnly));
        assert_eq!(prefs.steering, Some(SteeringChoice::WorkspaceOnly));
        assert_eq!(prefs.global_intensity, Some(SteeringIntensity::Lite));
        assert_eq!(prefs.workspace_intensity, Some(SteeringIntensity::Full));
        assert_eq!(
            prefs.selected_ides,
            Some(vec!["cursor".to_string(), "kiro".to_string()])
        );

        let workspaces = prefs.activated_workspaces.unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(
            workspaces[0].path,
            PathBuf::from("/home/user/projects/my-app")
        );
        assert_eq!(workspaces[0].activated_at, "2024-12-01T10:30:00Z");
        assert_eq!(workspaces[0].steering_intensity, SteeringIntensity::Full);
    }

    #[test]
    fn test_load_partial_toml() {
        // Only some keys present — missing ones should be None
        let toml_content = r#"
scope = "global"
"#;
        let prefs: InstallPreferences = toml::from_str(toml_content).unwrap();
        assert_eq!(prefs.scope, Some(InstallScope::Global));
        assert_eq!(prefs.steering, None);
        assert_eq!(prefs.global_intensity, None);
        assert_eq!(prefs.workspace_intensity, None);
        assert_eq!(prefs.selected_ides, None);
        assert_eq!(prefs.activated_workspaces, None);
    }

    #[test]
    fn test_save_and_load_roundtrip_on_disk() {
        let tmp_dir = TempDir::new().unwrap();
        let prefs_path = tmp_dir
            .path()
            .join("codryn")
            .join("install-preferences.toml");

        let prefs = InstallPreferences {
            scope: Some(InstallScope::Both),
            steering: Some(SteeringChoice::Yes),
            global_intensity: Some(SteeringIntensity::Full),
            workspace_intensity: Some(SteeringIntensity::Lite),
            selected_ides: Some(vec!["vscode".to_string()]),
            activated_workspaces: None,
        };

        // Save manually to the tmp path (since path() returns system config dir)
        let parent = prefs_path.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        let contents = toml::to_string_pretty(&prefs).unwrap();
        fs::write(&prefs_path, &contents).unwrap();

        // Load from file
        let loaded_contents = fs::read_to_string(&prefs_path).unwrap();
        let loaded: InstallPreferences = toml::from_str(&loaded_contents).unwrap();

        assert_eq!(loaded.scope, prefs.scope);
        assert_eq!(loaded.steering, prefs.steering);
        assert_eq!(loaded.global_intensity, prefs.global_intensity);
        assert_eq!(loaded.workspace_intensity, prefs.workspace_intensity);
        assert_eq!(loaded.selected_ides, prefs.selected_ides);
    }

    #[test]
    fn test_path_returns_expected_location() {
        let path = InstallPreferences::path();
        assert!(path.ends_with("codryn/install-preferences.toml"));
    }

    #[test]
    fn test_enum_serde_kebab_case() {
        // Verify kebab-case serialization
        assert_eq!(
            serde_json::to_string(&InstallScope::WorkspaceOnly).unwrap(),
            "\"workspace-only\""
        );
        assert_eq!(
            serde_json::to_string(&SteeringIntensity::Full).unwrap(),
            "\"full\""
        );
        assert_eq!(
            serde_json::to_string(&SteeringChoice::WorkspaceOnly).unwrap(),
            "\"workspace-only\""
        );
    }
}
