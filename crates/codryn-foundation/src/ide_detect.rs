//! IDE detection module.
//!
//! Detects installed IDEs and agents by checking for their configuration
//! directories, application bundles, and CLI binaries. Provides platform-specific
//! path resolution for each IDE's config location and MCP config file path.

use std::path::{Path, PathBuf};

use crate::platform;

/// Supported IDE/agent identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ide {
    VsCode,
    Cursor,
    Kiro,
    Windsurf,
    ClaudeDesktop,
    ClaudeCode,
    Zed,
    Codex,
    Gemini,
}

impl Ide {
    /// Human-readable display name for the IDE.
    pub fn display_name(&self) -> &'static str {
        match self {
            Ide::VsCode => "VS Code",
            Ide::Cursor => "Cursor",
            Ide::Kiro => "Kiro",
            Ide::Windsurf => "Windsurf",
            Ide::ClaudeDesktop => "Claude Desktop",
            Ide::ClaudeCode => "Claude Code",
            Ide::Zed => "Zed",
            Ide::Codex => "Codex",
            Ide::Gemini => "Gemini",
        }
    }

    /// Serialization key used in preferences.
    pub fn key(&self) -> &'static str {
        match self {
            Ide::VsCode => "vscode",
            Ide::Cursor => "cursor",
            Ide::Kiro => "kiro",
            Ide::Windsurf => "windsurf",
            Ide::ClaudeDesktop => "claude-desktop",
            Ide::ClaudeCode => "claude-code",
            Ide::Zed => "zed",
            Ide::Codex => "codex",
            Ide::Gemini => "gemini",
        }
    }
}

/// Result of IDE detection.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedIde {
    /// Which IDE was detected.
    pub ide: Ide,
    /// The IDE's configuration directory.
    pub config_dir: PathBuf,
    /// Path to the IDE's MCP configuration file.
    pub mcp_config_path: PathBuf,
    /// How the IDE was detected: "directory", "app_bundle", or "cli_binary".
    pub detection_method: &'static str,
}

/// Detect all installed IDEs on the system.
///
/// Checks for each IDE's configuration directory, macOS application bundle,
/// and CLI binary in PATH. Returns one entry per detected IDE.
pub fn detect_ides() -> Vec<DetectedIde> {
    let Some(home) = platform::home_dir() else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    detect_ides_with_home(&home)
}

/// Detect all installed IDEs using the given home directory.
///
/// This is the testable core of `detect_ides()` — it accepts a custom home path
/// instead of relying on the real home directory. Used by property tests to
/// verify detection accuracy with synthetic filesystem state.
pub fn detect_ides_with_home(home: &Path) -> Vec<DetectedIde> {
    let mut detected = Vec::new();

    // Claude Desktop / Claude Code
    detect_claude(home, &mut detected);

    // VS Code
    detect_vscode(home, &mut detected);

    // Cursor
    detect_cursor(home, &mut detected);

    // Windsurf
    detect_windsurf(home, &mut detected);

    // Zed
    detect_zed(home, &mut detected);

    // Codex
    detect_codex(home, &mut detected);

    // Gemini
    detect_gemini(home, &mut detected);

    // Kiro
    detect_kiro(home, &mut detected);

    detected
}

// ── Per-IDE detection ────────────────────────────────────────────────────────

fn detect_claude(home: &Path, detected: &mut Vec<DetectedIde>) {
    // Claude Desktop config directory
    let config_dir = claude_config_dir(home);
    let mcp_config = config_dir.join("mcp_servers.json");

    if let Some(method) = detect_presence(&config_dir, "Claude", &["claude"]) {
        detected.push(DetectedIde {
            ide: Ide::ClaudeDesktop,
            config_dir: config_dir.clone(),
            mcp_config_path: mcp_config,
            detection_method: method,
        });
    }

    // Claude Code (CLI-only, uses same config dir but detected via binary)
    let claude_dir = home.join(".claude");
    if claude_dir.exists() || which("claude") {
        let method = if claude_dir.exists() {
            "directory"
        } else {
            "cli_binary"
        };
        detected.push(DetectedIde {
            ide: Ide::ClaudeCode,
            config_dir: claude_dir.clone(),
            mcp_config_path: claude_dir.join("mcp_servers.json"),
            detection_method: method,
        });
    }
}

fn detect_vscode(home: &Path, detected: &mut Vec<DetectedIde>) {
    let config_dir = vscode_config_dir(home);
    let mcp_config = config_dir.join("mcp.json");

    if let Some(method) = detect_presence(&config_dir, "Visual Studio Code", &["code"]) {
        detected.push(DetectedIde {
            ide: Ide::VsCode,
            config_dir,
            mcp_config_path: mcp_config,
            detection_method: method,
        });
    }
}

fn detect_cursor(home: &Path, detected: &mut Vec<DetectedIde>) {
    let config_dir = home.join(".cursor");
    let mcp_config = config_dir.join("mcp.json");

    if let Some(method) = detect_presence(&config_dir, "Cursor", &["cursor"]) {
        detected.push(DetectedIde {
            ide: Ide::Cursor,
            config_dir,
            mcp_config_path: mcp_config,
            detection_method: method,
        });
    }
}

fn detect_windsurf(home: &Path, detected: &mut Vec<DetectedIde>) {
    let config_dir = home.join(".codeium").join("windsurf");
    let mcp_config = config_dir.join("mcp.json");

    if let Some(method) = detect_presence(&config_dir, "Windsurf", &["windsurf"]) {
        detected.push(DetectedIde {
            ide: Ide::Windsurf,
            config_dir,
            mcp_config_path: mcp_config,
            detection_method: method,
        });
    }
}

fn detect_zed(home: &Path, detected: &mut Vec<DetectedIde>) {
    let config_dir = zed_config_dir(home);
    let mcp_config = config_dir.join("settings.json");

    if let Some(method) = detect_presence(&config_dir, "Zed", &["zed"]) {
        detected.push(DetectedIde {
            ide: Ide::Zed,
            config_dir,
            mcp_config_path: mcp_config,
            detection_method: method,
        });
    }
}

fn detect_codex(home: &Path, detected: &mut Vec<DetectedIde>) {
    let config_dir = home.join(".codex");
    let mcp_config = config_dir.join("config.toml");

    if config_dir.exists() || which("codex") {
        let method = if config_dir.exists() {
            "directory"
        } else {
            "cli_binary"
        };
        detected.push(DetectedIde {
            ide: Ide::Codex,
            config_dir,
            mcp_config_path: mcp_config,
            detection_method: method,
        });
    }
}

fn detect_gemini(home: &Path, detected: &mut Vec<DetectedIde>) {
    let config_dir = home.join(".gemini");
    let mcp_config = config_dir.join("mcp.json");

    if config_dir.exists() || which("gemini") {
        let method = if config_dir.exists() {
            "directory"
        } else {
            "cli_binary"
        };
        detected.push(DetectedIde {
            ide: Ide::Gemini,
            config_dir,
            mcp_config_path: mcp_config,
            detection_method: method,
        });
    }
}

fn detect_kiro(home: &Path, detected: &mut Vec<DetectedIde>) {
    let config_dir = home.join(".kiro");
    let mcp_config = config_dir.join("settings").join("mcp.json");

    if let Some(method) = detect_presence(&config_dir, "Kiro", &["kiro-cli", "kiro"]) {
        detected.push(DetectedIde {
            ide: Ide::Kiro,
            config_dir,
            mcp_config_path: mcp_config,
            detection_method: method,
        });
    }
}

// ── Platform-specific paths ──────────────────────────────────────────────────

/// Resolve Claude Desktop's config directory based on platform.
fn claude_config_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Claude")
    } else if cfg!(target_os = "windows") {
        // Use APPDATA on Windows
        std::env::var("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("Claude"))
            .unwrap_or_else(|_| home.join("AppData/Roaming/Claude"))
    } else {
        // Linux: XDG config
        std::env::var("XDG_CONFIG_HOME")
            .map(|xdg| PathBuf::from(xdg).join("claude"))
            .unwrap_or_else(|_| home.join(".config/claude"))
    }
}

/// Resolve VS Code's config directory based on platform.
fn vscode_config_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Code/User")
    } else if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("Code/User"))
            .unwrap_or_else(|_| home.join("AppData/Roaming/Code/User"))
    } else {
        // Linux: standard ~/.vscode or XDG
        home.join(".vscode")
    }
}

/// Resolve Zed's config directory based on platform.
fn zed_config_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Zed")
    } else if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("Zed"))
            .unwrap_or_else(|_| home.join("AppData/Roaming/Zed"))
    } else {
        home.join(".config/zed")
    }
}

// ── Utility helpers ──────────────────────────────────────────────────────────

/// Check if an IDE is present via config directory, app bundle, or CLI binary.
/// Returns the detection method if found, or None.
fn detect_presence(config_dir: &Path, app_name: &str, cli_names: &[&str]) -> Option<&'static str> {
    if config_dir.exists() {
        return Some("directory");
    }
    if app_exists(app_name) {
        return Some("app_bundle");
    }
    for cli in cli_names {
        if which(cli) {
            return Some("cli_binary");
        }
    }
    None
}

/// Check if a macOS .app bundle exists in /Applications or ~/Applications.
fn app_exists(name: &str) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let app = format!("{}.app", name);
    Path::new("/Applications").join(&app).exists()
        || platform::home_dir()
            .map(|h| PathBuf::from(h).join("Applications").join(&app).exists())
            .unwrap_or(false)
}

/// Check if a binary exists in PATH.
fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).exists()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ide_display_names() {
        assert_eq!(Ide::VsCode.display_name(), "VS Code");
        assert_eq!(Ide::Cursor.display_name(), "Cursor");
        assert_eq!(Ide::Kiro.display_name(), "Kiro");
        assert_eq!(Ide::Windsurf.display_name(), "Windsurf");
        assert_eq!(Ide::ClaudeDesktop.display_name(), "Claude Desktop");
        assert_eq!(Ide::ClaudeCode.display_name(), "Claude Code");
        assert_eq!(Ide::Zed.display_name(), "Zed");
        assert_eq!(Ide::Codex.display_name(), "Codex");
        assert_eq!(Ide::Gemini.display_name(), "Gemini");
    }

    #[test]
    fn test_ide_keys() {
        assert_eq!(Ide::VsCode.key(), "vscode");
        assert_eq!(Ide::Cursor.key(), "cursor");
        assert_eq!(Ide::Kiro.key(), "kiro");
        assert_eq!(Ide::Windsurf.key(), "windsurf");
        assert_eq!(Ide::ClaudeDesktop.key(), "claude-desktop");
        assert_eq!(Ide::ClaudeCode.key(), "claude-code");
        assert_eq!(Ide::Zed.key(), "zed");
        assert_eq!(Ide::Codex.key(), "codex");
        assert_eq!(Ide::Gemini.key(), "gemini");
    }

    #[test]
    fn test_detect_presence_with_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("test-ide");
        std::fs::create_dir_all(&config_dir).unwrap();

        let result = detect_presence(&config_dir, "NonExistentApp", &["nonexistent-bin"]);
        assert_eq!(result, Some("directory"));
    }

    #[test]
    fn test_detect_presence_nonexistent() {
        let result = detect_presence(
            Path::new("/nonexistent/path/that/does/not/exist"),
            "NonExistentApp99999",
            &["nonexistent-binary-99999"],
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_detect_ides_with_temp_dirs() {
        // Create a temporary home directory with some IDE config dirs
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        // Create a .cursor config dir
        std::fs::create_dir_all(home.join(".cursor")).unwrap();

        // Create a .gemini config dir
        std::fs::create_dir_all(home.join(".gemini")).unwrap();

        // Manually test the per-IDE detection functions
        let mut detected = Vec::new();
        detect_cursor(home, &mut detected);
        detect_gemini(home, &mut detected);

        assert_eq!(detected.len(), 2);
        assert_eq!(detected[0].ide, Ide::Cursor);
        assert_eq!(detected[0].detection_method, "directory");
        assert_eq!(detected[0].config_dir, home.join(".cursor"));
        assert_eq!(detected[0].mcp_config_path, home.join(".cursor/mcp.json"));

        assert_eq!(detected[1].ide, Ide::Gemini);
        assert_eq!(detected[1].detection_method, "directory");
        assert_eq!(detected[1].config_dir, home.join(".gemini"));
        assert_eq!(detected[1].mcp_config_path, home.join(".gemini/mcp.json"));
    }

    #[test]
    fn test_detect_kiro_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        std::fs::create_dir_all(home.join(".kiro")).unwrap();

        let mut detected = Vec::new();
        detect_kiro(home, &mut detected);

        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].ide, Ide::Kiro);
        assert_eq!(detected[0].config_dir, home.join(".kiro"));
        assert_eq!(
            detected[0].mcp_config_path,
            home.join(".kiro/settings/mcp.json")
        );
    }

    #[test]
    fn test_detect_windsurf_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        std::fs::create_dir_all(home.join(".codeium/windsurf")).unwrap();

        let mut detected = Vec::new();
        detect_windsurf(home, &mut detected);

        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].ide, Ide::Windsurf);
        assert_eq!(detected[0].config_dir, home.join(".codeium/windsurf"));
        assert_eq!(
            detected[0].mcp_config_path,
            home.join(".codeium/windsurf/mcp.json")
        );
    }

    #[test]
    fn test_detect_codex_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        std::fs::create_dir_all(home.join(".codex")).unwrap();

        let mut detected = Vec::new();
        detect_codex(home, &mut detected);

        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].ide, Ide::Codex);
        assert_eq!(detected[0].config_dir, home.join(".codex"));
        assert_eq!(detected[0].mcp_config_path, home.join(".codex/config.toml"));
    }

    #[test]
    fn test_no_ides_detected_with_nonexistent_dirs() {
        // Verify detect_presence returns None for paths that don't exist
        // and apps/CLIs that are not present on the system.
        let result = detect_presence(
            Path::new("/tmp/nonexistent_ide_dir_test_12345"),
            "NonExistentApp12345",
            &["nonexistent_cli_12345"],
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_detect_functions_with_empty_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        // With an empty home directory (no IDE config dirs created),
        // directory-based detection should find nothing.
        // Note: app_exists() and which() check real system state,
        // so we only assert directory-based detection here.
        let mut detected = Vec::new();
        detect_codex(home, &mut detected);
        detect_gemini(home, &mut detected);

        // These two only check config_dir.exists() || which(),
        // and with a temp home neither .codex nor .gemini exist.
        // which("codex") and which("gemini") may or may not find binaries
        // depending on the system, so filter to directory-detected only.
        let dir_detected: Vec<_> = detected
            .iter()
            .filter(|d| d.detection_method == "directory")
            .collect();
        assert!(dir_detected.is_empty());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;

    /// **Validates: Requirements 1.2**
    ///
    /// **Property 2: IDE Detection Accuracy**
    /// For any subset of IDE configuration directories present on the filesystem,
    /// `detect_ides_with_home()` returns exactly the IDEs whose config directories
    /// exist, with no false positives (reporting IDEs not installed) and no false
    /// negatives (missing IDEs that are installed), for directory-based detection.
    /// Returns the config directory path relative to home for each IDE.
    /// These are the directories that, when present, trigger directory-based detection.
    fn ide_config_dirs() -> Vec<(Ide, Vec<&'static str>)> {
        // For platform-specific IDEs, use the current platform's paths.
        // On macOS: Claude Desktop = Library/Application Support/Claude
        //           VS Code = Library/Application Support/Code/User
        //           Zed = Library/Application Support/Zed
        // On Linux:  Claude Desktop = .config/claude
        //           VS Code = .vscode
        //           Zed = .config/zed
        let mut configs = vec![
            (Ide::Cursor, vec![".cursor"]),
            (Ide::Kiro, vec![".kiro"]),
            (Ide::Windsurf, vec![".codeium/windsurf"]),
            (Ide::Codex, vec![".codex"]),
            (Ide::Gemini, vec![".gemini"]),
            (Ide::ClaudeCode, vec![".claude"]),
        ];

        // Platform-specific paths
        if cfg!(target_os = "macos") {
            configs.push((
                Ide::ClaudeDesktop,
                vec!["Library/Application Support/Claude"],
            ));
            configs.push((Ide::VsCode, vec!["Library/Application Support/Code/User"]));
            configs.push((Ide::Zed, vec!["Library/Application Support/Zed"]));
        } else if cfg!(target_os = "linux") {
            configs.push((Ide::ClaudeDesktop, vec![".config/claude"]));
            configs.push((Ide::VsCode, vec![".vscode"]));
            configs.push((Ide::Zed, vec![".config/zed"]));
        } else {
            // Windows: skip these for now, test focuses on unix platforms
            configs.push((Ide::ClaudeDesktop, vec![".config/claude"]));
            configs.push((Ide::VsCode, vec![".vscode"]));
            configs.push((Ide::Zed, vec![".config/zed"]));
        }

        configs
    }

    /// Strategy: generate a random boolean for each IDE indicating presence.
    fn ide_presence_strategy() -> impl Strategy<Value = Vec<bool>> {
        let count = ide_config_dirs().len();
        proptest::collection::vec(proptest::bool::ANY, count..=count)
    }

    proptest! {
        #[test]
        fn prop_ide_detection_accuracy(presence in ide_presence_strategy()) {
            let configs = ide_config_dirs();
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();

            // Create directories for IDEs marked as present
            let mut expected_ides: HashSet<Ide> = HashSet::new();
            for (i, (ide, paths)) in configs.iter().enumerate() {
                if presence[i] {
                    for path in paths {
                        let full_path = home.join(path);
                        std::fs::create_dir_all(&full_path).unwrap();
                    }
                    expected_ides.insert(ide.clone());
                }
            }

            // Run detection with our synthetic home
            let detected = detect_ides_with_home(home);

            // Filter to only directory-detected IDEs to avoid interference
            // from app_exists() and which() checking real system state
            let dir_detected: HashSet<Ide> = detected
                .iter()
                .filter(|d| d.detection_method == "directory")
                .map(|d| d.ide.clone())
                .collect();

            // Property: No false positives for directory detection
            // Every IDE detected via "directory" must have its config dir present
            for ide in &dir_detected {
                prop_assert!(
                    expected_ides.contains(ide),
                    "False positive: {:?} detected but config dir was not created",
                    ide
                );
            }

            // Property: No false negatives for directory detection
            // Every IDE whose config dir exists must be detected (at least via directory)
            for ide in &expected_ides {
                prop_assert!(
                    dir_detected.contains(ide),
                    "False negative: {:?} config dir exists but was not detected",
                    ide
                );
            }

            // Stronger assertion: the sets are exactly equal
            prop_assert_eq!(
                dir_detected,
                expected_ides,
                "Directory-detected IDEs do not exactly match expected set"
            );
        }
    }
}
