use codryn_foundation::config::AppConfig;
use tracing_subscriber::fmt;
use tracing_subscriber::EnvFilter;

/// Initialize structured logging based on configuration.
///
/// Reads logging configuration from the provided `AppConfig` (if any),
/// falling back to environment variables `CBM_LOG_FORMAT` and `CODRYN_LOG_LEVEL`.
///
/// - `CBM_LOG_FORMAT=json` enables JSON output with timestamp, level, module,
///   message, and span context.
/// - Any other value (or unset) uses a human-readable compact format (default).
///
/// `CODRYN_LOG_LEVEL` supports `tracing_subscriber::EnvFilter` syntax, including
/// per-module overrides such as `info,codryn_pipeline=debug,codryn_store=warn`.
///
/// All log output is directed to stderr so that stdout remains available for
/// MCP JSON-RPC communication.
pub fn init_logging(config: Option<&AppConfig>) {
    let format = resolve_format(config);
    let filter = resolve_filter(config);

    let env_filter = EnvFilter::try_new(&filter).unwrap_or_else(|_| {
        eprintln!(
            "[codryn] Invalid CODRYN_LOG_LEVEL value '{}'; falling back to 'info'",
            filter
        );
        EnvFilter::new("info")
    });

    match format.as_str() {
        "json" => {
            fmt()
                .json()
                .with_target(true)
                .with_level(true)
                .with_span_events(fmt::format::FmtSpan::CLOSE)
                .with_env_filter(env_filter)
                .with_writer(std::io::stderr)
                .init();
        }
        _ => {
            fmt()
                .compact()
                .with_target(true)
                .with_level(true)
                .with_env_filter(env_filter)
                .with_writer(std::io::stderr)
                .init();
        }
    }
}

/// Resolve the log format from config or environment variable.
/// Priority: env var `CBM_LOG_FORMAT` > `AppConfig.log_format` > "text"
fn resolve_format(config: Option<&AppConfig>) -> String {
    std::env::var("CBM_LOG_FORMAT")
        .ok()
        .or_else(|| config.and_then(|c| c.log_format.clone()))
        .unwrap_or_else(|| "text".to_string())
}

/// Resolve the log filter/level from config or environment variable.
/// Priority: env var `CODRYN_LOG_LEVEL` > `AppConfig.log_level` > "info"
fn resolve_filter(config: Option<&AppConfig>) -> String {
    std::env::var("CODRYN_LOG_LEVEL")
        .ok()
        .or_else(|| config.and_then(|c| c.log_level.clone()))
        .unwrap_or_else(|| "info".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex to serialize tests that modify environment variables
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_resolve_format_defaults_to_text() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("CBM_LOG_FORMAT");
        let result = resolve_format(None);
        assert_eq!(result, "text");
    }

    #[test]
    fn test_resolve_format_from_config() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("CBM_LOG_FORMAT");
        let config = AppConfig {
            log_format: Some("json".to_string()),
            ..Default::default()
        };
        let result = resolve_format(Some(&config));
        assert_eq!(result, "json");
    }

    #[test]
    fn test_resolve_format_env_overrides_config() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("CBM_LOG_FORMAT", "json");
        let config = AppConfig {
            log_format: Some("text".to_string()),
            ..Default::default()
        };
        let result = resolve_format(Some(&config));
        std::env::remove_var("CBM_LOG_FORMAT");
        assert_eq!(result, "json");
    }

    #[test]
    fn test_resolve_filter_defaults_to_info() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("CODRYN_LOG_LEVEL");
        let result = resolve_filter(None);
        assert_eq!(result, "info");
    }

    #[test]
    fn test_resolve_filter_from_config() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("CODRYN_LOG_LEVEL");
        let config = AppConfig {
            log_level: Some("debug".to_string()),
            ..Default::default()
        };
        let result = resolve_filter(Some(&config));
        assert_eq!(result, "debug");
    }

    #[test]
    fn test_resolve_filter_env_overrides_config() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("CODRYN_LOG_LEVEL", "codryn_pipeline=debug,codryn_store=warn");
        let config = AppConfig {
            log_level: Some("info".to_string()),
            ..Default::default()
        };
        let result = resolve_filter(Some(&config));
        std::env::remove_var("CODRYN_LOG_LEVEL");
        assert_eq!(result, "codryn_pipeline=debug,codryn_store=warn");
    }

    #[test]
    fn test_resolve_filter_supports_per_module_overrides() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("CODRYN_LOG_LEVEL");
        let config = AppConfig {
            log_level: Some("info,codryn_pipeline=debug,codryn_store=warn".to_string()),
            ..Default::default()
        };
        let result = resolve_filter(Some(&config));
        assert_eq!(result, "info,codryn_pipeline=debug,codryn_store=warn");

        // Verify it parses as a valid EnvFilter
        let filter = EnvFilter::try_new(&result);
        assert!(
            filter.is_ok(),
            "Filter string should be valid EnvFilter syntax"
        );
    }
}
