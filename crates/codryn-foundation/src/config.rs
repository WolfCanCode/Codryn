use serde::Deserialize;
use std::path::PathBuf;
use tracing::warn;

/// Application configuration loaded from TOML file + environment overrides.
#[derive(Deserialize, Default, Debug, Clone)]
pub struct AppConfig {
    pub store_path: Option<PathBuf>,
    pub log_level: Option<String>,
    pub log_format: Option<String>,
    pub pool_size: Option<usize>,
    pub staleness_threshold_secs: Option<u64>,
    pub max_memory_mb: Option<u64>,
    pub rate_limit: Option<RateLimitConfig>,
    pub ui_port: Option<u16>,
    pub plugin_dir: Option<PathBuf>,
    pub snapshot_retention: Option<usize>,
}

/// Rate limiting configuration.
#[derive(Deserialize, Default, Debug, Clone)]
pub struct RateLimitConfig {
    pub window_secs: Option<u64>,
    pub max_expensive: Option<usize>,
    pub threshold_ms: Option<u64>,
}

/// Intermediate TOML structure that maps the nested config file format
/// to the flat `AppConfig` struct.
#[derive(Deserialize, Default)]
struct RawConfig {
    store_path: Option<String>,
    log_level: Option<String>,
    log_format: Option<String>,
    pool: Option<RawPoolConfig>,
    indexing: Option<RawIndexingConfig>,
    rate_limit: Option<RateLimitConfig>,
    snapshots: Option<RawSnapshotsConfig>,
    ui: Option<RawUiConfig>,
    plugin_dir: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawPoolConfig {
    max_readers: Option<usize>,
}

#[derive(Deserialize, Default)]
struct RawIndexingConfig {
    staleness_threshold_secs: Option<u64>,
    max_memory_mb: Option<u64>,
}

#[derive(Deserialize, Default)]
struct RawSnapshotsConfig {
    retention: Option<usize>,
}

#[derive(Deserialize, Default)]
struct RawUiConfig {
    port: Option<u16>,
}

impl AppConfig {
    /// Load configuration from `~/.config/codryn/config.toml`, then apply
    /// environment variable overrides. Returns defaults if the file is
    /// missing or contains invalid values.
    pub fn load() -> Self {
        let mut config = Self::load_from_file();
        config.apply_env_overrides();
        config
    }

    /// Generate a default config file template with comments.
    pub fn generate_default() -> String {
        r#"# CBM Configuration
# Place this file at ~/.config/codryn/config.toml

# Path to the graph database store
# store_path = "~/.local/share/codryn"

# Logging level: trace, debug, info, warn, error
# log_level = "info"

# Log format: "text" or "json"
# log_format = "text"

# Plugin directory for custom pipeline passes
# plugin_dir = "~/.local/share/codryn/plugins"

[pool]
# Number of concurrent read connections
# max_readers = 4

[indexing]
# Seconds before a file is considered stale
# staleness_threshold_secs = 3600

# Maximum memory usage in MB during indexing
# max_memory_mb = 2048

[rate_limit]
# Sliding window duration in seconds
# window_secs = 60

# Maximum expensive queries per window
# max_expensive = 10

# Threshold in ms for a query to be considered expensive
# threshold_ms = 500

[snapshots]
# Number of historical snapshots to retain per project
# retention = 10

[ui]
# Port for the UI dashboard
# port = 3000
"#
        .to_string()
    }

    /// Resolve the config file path: `~/.config/codryn/config.toml`
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("codryn").join("config.toml"))
    }

    /// Load from the TOML file, returning defaults if the file doesn't exist
    /// or contains parse errors.
    fn load_from_file() -> Self {
        let path = match Self::config_path() {
            Some(p) => p,
            None => {
                warn!("Could not determine config directory; using defaults");
                return Self::default();
            }
        };

        if !path.exists() {
            return Self::default();
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    "Failed to read config file {}: {}; using defaults",
                    path.display(),
                    e
                );
                return Self::default();
            }
        };

        let raw: RawConfig = match toml::from_str(&content) {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    "Invalid config file {}: {}; using defaults",
                    path.display(),
                    e
                );
                return Self::default();
            }
        };

        Self::from_raw(raw)
    }

    /// Convert the raw nested TOML structure into the flat AppConfig.
    fn from_raw(raw: RawConfig) -> Self {
        Self {
            store_path: raw.store_path.map(PathBuf::from),
            log_level: raw.log_level,
            log_format: raw.log_format,
            pool_size: raw.pool.and_then(|p| p.max_readers),
            staleness_threshold_secs: raw
                .indexing
                .as_ref()
                .and_then(|i| i.staleness_threshold_secs),
            max_memory_mb: raw.indexing.and_then(|i| i.max_memory_mb),
            rate_limit: raw.rate_limit,
            ui_port: raw.ui.and_then(|u| u.port),
            plugin_dir: raw.plugin_dir.map(PathBuf::from),
            snapshot_retention: raw.snapshots.and_then(|s| s.retention),
        }
    }

    /// Apply environment variable overrides. Env vars take precedence over
    /// file-based config values.
    fn apply_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("CODRYN_STORE_PATH") {
            self.store_path = Some(PathBuf::from(val));
        }

        if let Ok(val) = std::env::var("CBM_LOG_LEVEL") {
            self.log_level = Some(val);
        }

        if let Ok(val) = std::env::var("CBM_LOG_FORMAT") {
            self.log_format = Some(val);
        }

        if let Ok(val) = std::env::var("CBM_POOL_SIZE") {
            match val.parse::<usize>() {
                Ok(n) => self.pool_size = Some(n),
                Err(_) => warn!("Invalid CBM_POOL_SIZE value '{}'; ignoring", val),
            }
        }

        if let Ok(val) = std::env::var("CBM_STALENESS_THRESHOLD_SECS") {
            match val.parse::<u64>() {
                Ok(n) => self.staleness_threshold_secs = Some(n),
                Err(_) => warn!(
                    "Invalid CBM_STALENESS_THRESHOLD_SECS value '{}'; ignoring",
                    val
                ),
            }
        }

        if let Ok(val) = std::env::var("CBM_MAX_MEMORY_MB") {
            match val.parse::<u64>() {
                Ok(n) => self.max_memory_mb = Some(n),
                Err(_) => warn!("Invalid CBM_MAX_MEMORY_MB value '{}'; ignoring", val),
            }
        }

        if let Ok(val) = std::env::var("CBM_UI_PORT") {
            match val.parse::<u16>() {
                Ok(n) => self.ui_port = Some(n),
                Err(_) => warn!("Invalid CBM_UI_PORT value '{}'; ignoring", val),
            }
        }

        if let Ok(val) = std::env::var("CBM_PLUGIN_DIR") {
            self.plugin_dir = Some(PathBuf::from(val));
        }

        if let Ok(val) = std::env::var("CBM_SNAPSHOT_RETENTION") {
            match val.parse::<usize>() {
                Ok(n) => self.snapshot_retention = Some(n),
                Err(_) => warn!("Invalid CBM_SNAPSHOT_RETENTION value '{}'; ignoring", val),
            }
        }

        // Rate limit overrides
        if let Ok(val) = std::env::var("CBM_RATE_LIMIT_WINDOW_SECS") {
            match val.parse::<u64>() {
                Ok(n) => {
                    self.rate_limit
                        .get_or_insert_with(RateLimitConfig::default)
                        .window_secs = Some(n);
                }
                Err(_) => warn!(
                    "Invalid CBM_RATE_LIMIT_WINDOW_SECS value '{}'; ignoring",
                    val
                ),
            }
        }

        if let Ok(val) = std::env::var("CBM_RATE_LIMIT_MAX_EXPENSIVE") {
            match val.parse::<usize>() {
                Ok(n) => {
                    self.rate_limit
                        .get_or_insert_with(RateLimitConfig::default)
                        .max_expensive = Some(n);
                }
                Err(_) => warn!(
                    "Invalid CBM_RATE_LIMIT_MAX_EXPENSIVE value '{}'; ignoring",
                    val
                ),
            }
        }

        if let Ok(val) = std::env::var("CBM_RATE_LIMIT_THRESHOLD_MS") {
            match val.parse::<u64>() {
                Ok(n) => {
                    self.rate_limit
                        .get_or_insert_with(RateLimitConfig::default)
                        .threshold_ms = Some(n);
                }
                Err(_) => warn!(
                    "Invalid CBM_RATE_LIMIT_THRESHOLD_MS value '{}'; ignoring",
                    val
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex to serialize tests that modify environment variables
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert!(config.store_path.is_none());
        assert!(config.log_level.is_none());
        assert!(config.log_format.is_none());
        assert!(config.pool_size.is_none());
        assert!(config.rate_limit.is_none());
    }

    #[test]
    fn test_generate_default_contains_sections() {
        let template = AppConfig::generate_default();
        assert!(template.contains("[pool]"));
        assert!(template.contains("[indexing]"));
        assert!(template.contains("[rate_limit]"));
        assert!(template.contains("[snapshots]"));
        assert!(template.contains("[ui]"));
        assert!(template.contains("store_path"));
        assert!(template.contains("log_level"));
    }

    #[test]
    fn test_from_raw_full_config() {
        let raw = RawConfig {
            store_path: Some("~/.local/share/codryn".to_string()),
            log_level: Some("debug".to_string()),
            log_format: Some("json".to_string()),
            pool: Some(RawPoolConfig {
                max_readers: Some(8),
            }),
            indexing: Some(RawIndexingConfig {
                staleness_threshold_secs: Some(7200),
                max_memory_mb: Some(4096),
            }),
            rate_limit: Some(RateLimitConfig {
                window_secs: Some(120),
                max_expensive: Some(20),
                threshold_ms: Some(1000),
            }),
            snapshots: Some(RawSnapshotsConfig { retention: Some(5) }),
            ui: Some(RawUiConfig { port: Some(8080) }),
            plugin_dir: Some("/opt/codryn/plugins".to_string()),
        };

        let config = AppConfig::from_raw(raw);
        assert_eq!(
            config.store_path,
            Some(PathBuf::from("~/.local/share/codryn"))
        );
        assert_eq!(config.log_level.as_deref(), Some("debug"));
        assert_eq!(config.log_format.as_deref(), Some("json"));
        assert_eq!(config.pool_size, Some(8));
        assert_eq!(config.staleness_threshold_secs, Some(7200));
        assert_eq!(config.max_memory_mb, Some(4096));
        assert_eq!(config.ui_port, Some(8080));
        assert_eq!(
            config.plugin_dir,
            Some(PathBuf::from("/opt/codryn/plugins"))
        );
        assert_eq!(config.snapshot_retention, Some(5));

        let rl = config.rate_limit.unwrap();
        assert_eq!(rl.window_secs, Some(120));
        assert_eq!(rl.max_expensive, Some(20));
        assert_eq!(rl.threshold_ms, Some(1000));
    }

    #[test]
    fn test_env_overrides() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // Set env vars
        std::env::set_var("CODRYN_STORE_PATH", "/tmp/codryn-test");
        std::env::set_var("CBM_LOG_LEVEL", "trace");
        std::env::set_var("CBM_LOG_FORMAT", "json");
        std::env::set_var("CBM_POOL_SIZE", "16");
        std::env::set_var("CBM_UI_PORT", "9090");
        std::env::set_var("CBM_SNAPSHOT_RETENTION", "20");

        let mut config = AppConfig::default();
        config.apply_env_overrides();

        // Clean up before assertions to avoid leaking on panic
        std::env::remove_var("CODRYN_STORE_PATH");
        std::env::remove_var("CBM_LOG_LEVEL");
        std::env::remove_var("CBM_LOG_FORMAT");
        std::env::remove_var("CBM_POOL_SIZE");
        std::env::remove_var("CBM_UI_PORT");
        std::env::remove_var("CBM_SNAPSHOT_RETENTION");

        assert_eq!(config.store_path, Some(PathBuf::from("/tmp/codryn-test")));
        assert_eq!(config.log_level.as_deref(), Some("trace"));
        assert_eq!(config.log_format.as_deref(), Some("json"));
        assert_eq!(config.pool_size, Some(16));
        assert_eq!(config.ui_port, Some(9090));
        assert_eq!(config.snapshot_retention, Some(20));
    }

    #[test]
    fn test_invalid_env_values_ignored() {
        let _lock = ENV_MUTEX.lock().unwrap();

        std::env::set_var("CBM_POOL_SIZE", "not_a_number");
        std::env::set_var("CBM_UI_PORT", "99999");

        let mut config = AppConfig::default();
        config.apply_env_overrides();

        // Clean up before assertions
        std::env::remove_var("CBM_POOL_SIZE");
        std::env::remove_var("CBM_UI_PORT");

        // pool_size should remain None (invalid parse)
        assert!(config.pool_size.is_none());
        // ui_port should remain None (99999 > u16::MAX)
        assert!(config.ui_port.is_none());
    }

    #[test]
    fn test_parse_valid_toml() {
        let toml_content = r#"
store_path = "~/.local/share/codryn"
log_level = "info"
log_format = "text"

[pool]
max_readers = 4

[indexing]
staleness_threshold_secs = 3600
max_memory_mb = 2048

[rate_limit]
window_secs = 60
max_expensive = 10
threshold_ms = 500

[snapshots]
retention = 10

[ui]
port = 3000
"#;

        let raw: RawConfig = toml::from_str(toml_content).unwrap();
        let config = AppConfig::from_raw(raw);

        assert_eq!(
            config.store_path,
            Some(PathBuf::from("~/.local/share/codryn"))
        );
        assert_eq!(config.log_level.as_deref(), Some("info"));
        assert_eq!(config.log_format.as_deref(), Some("text"));
        assert_eq!(config.pool_size, Some(4));
        assert_eq!(config.staleness_threshold_secs, Some(3600));
        assert_eq!(config.max_memory_mb, Some(2048));
        assert_eq!(config.ui_port, Some(3000));
        assert_eq!(config.snapshot_retention, Some(10));

        let rl = config.rate_limit.unwrap();
        assert_eq!(rl.window_secs, Some(60));
        assert_eq!(rl.max_expensive, Some(10));
        assert_eq!(rl.threshold_ms, Some(500));
    }

    #[test]
    fn test_partial_toml_uses_none_for_missing() {
        let toml_content = r#"
log_level = "warn"
"#;

        let raw: RawConfig = toml::from_str(toml_content).unwrap();
        let config = AppConfig::from_raw(raw);

        assert_eq!(config.log_level.as_deref(), Some("warn"));
        assert!(config.store_path.is_none());
        assert!(config.pool_size.is_none());
        assert!(config.rate_limit.is_none());
    }

    #[test]
    fn test_env_overrides_take_precedence_over_file() {
        let _lock = ENV_MUTEX.lock().unwrap();

        let toml_content = r#"
log_level = "info"
"#;

        let raw: RawConfig = toml::from_str(toml_content).unwrap();
        let mut config = AppConfig::from_raw(raw);

        std::env::set_var("CBM_LOG_LEVEL", "error");
        config.apply_env_overrides();
        std::env::remove_var("CBM_LOG_LEVEL");

        assert_eq!(config.log_level.as_deref(), Some("error"));
    }
}
